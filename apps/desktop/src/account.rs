//! The Linger account, for Settings › Account and Ask: whether this device is signed in, the plan,
//! and what is left of it. Calls to the server run on threads; [`Account::poll`] collects their
//! results each frame.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use eframe::egui;
use linger_core::api::{self, Api, ApiError, Me};
use linger_core::auth::{self, OAuthConfig, OAuthSession, TokenSource};
use linger_core::secrets::SecretStore;

pub struct Account {
    store: SecretStore,
    /// The proxy as the signed-in user; `None` when signed out.
    api: Option<Api>,
    pub me: Option<Me>,
    /// What is under way ("Waiting for the browser…"), for the Account section.
    pub busy: Option<&'static str>,
    /// The last thing that went wrong, shown in the Account section.
    pub error: Option<String>,
    /// Checkout or the portal is open in the browser: look again when the window has focus back.
    awaiting_browser: bool,
    was_focused: bool,
    /// Calls in flight; results of an abandoned sign-in (an older `attempt`) are dropped.
    pending: usize,
    attempt: u64,
    tx: Sender<Done>,
    rx: Receiver<Done>,
}

enum Done {
    SignedIn(u64, Result<Arc<dyn TokenSource>, String>),
    Me(Result<Me, ApiError>),
    Opened(Result<String, ApiError>),
    Deleted(Result<(), ApiError>),
    SignedOut,
}

impl Account {
    /// The account this device last signed in to, if any; its details load in the background.
    pub fn new(data_dir: &Path) -> Self {
        let (tx, rx) = channel();
        let store = SecretStore::new(data_dir);
        let api = auth::saved(&store).map(|t| Api::new(api::base_url(), t));
        let mut account = Self {
            store,
            api,
            me: None,
            busy: None,
            error: None,
            awaiting_browser: false,
            was_focused: true,
            pending: 0,
            attempt: 0,
            tx,
            rx,
        };
        account.refresh();
        account
    }

    pub fn api(&self) -> Option<&Api> {
        self.api.as_ref()
    }

    pub fn signed_in(&self) -> bool {
        self.api.is_some()
    }

    fn spawn(&mut self, f: impl FnOnce() -> Done + Send + 'static) {
        self.pending += 1;
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("account".into())
            .spawn(move || {
                let _ = tx.send(f());
            })
            .expect("spawn account thread");
    }

    /// Opens Clerk's sign-in in the browser and waits for it to come back.
    pub fn sign_in(&mut self) {
        self.attempt += 1;
        self.busy = Some("Waiting for the browser…");
        self.error = None;
        let (store, attempt) = (self.store.clone(), self.attempt);
        self.spawn(move || {
            let r = OAuthSession::sign_in(OAuthConfig::from_env(), store, crate::app::open_url)
                .map(|s| Arc::new(s) as Arc<dyn TokenSource>)
                .map_err(|e| format!("{e:#}"));
            Done::SignedIn(attempt, r)
        });
    }

    /// Stops waiting for a browser sign-in (its listener gives up on its own later).
    pub fn cancel_sign_in(&mut self) {
        self.attempt += 1;
        self.busy = None;
    }

    pub fn sign_out(&mut self) {
        self.api = None;
        self.me = None;
        self.error = None;
        let store = self.store.clone();
        // Revoking needs the network; this device is signed out either way.
        self.spawn(move || {
            let _ = auth::sign_out(&store);
            Done::SignedOut
        });
    }

    /// Fetches the plan and allowance again.
    pub fn refresh(&mut self) {
        if let Some(api) = self.api.clone() {
            self.spawn(move || Done::Me(api.me()));
        }
    }

    /// Opens Stripe Checkout for Pro, monthly or yearly.
    pub fn upgrade(&mut self, yearly: bool) {
        if let Some(api) = self.api.clone() {
            self.busy = Some("Opening checkout…");
            self.spawn(move || Done::Opened(api.checkout(yearly)));
        }
    }

    /// Opens the Stripe Customer Portal.
    pub fn manage(&mut self) {
        if let Some(api) = self.api.clone() {
            self.busy = Some("Opening your subscription…");
            self.spawn(move || Done::Opened(api.portal()));
        }
    }

    pub fn delete(&mut self) {
        if let Some(api) = self.api.clone() {
            self.busy = Some("Deleting your account…");
            self.spawn(move || Done::Deleted(api.delete_account()));
        }
    }

    /// Development builds: signs in with a token pasted into Settings (empty: forgets it).
    pub fn set_dev_token(&mut self, token: &str) {
        let token = token.trim();
        let saved = if token.is_empty() {
            self.store.delete(auth::DEV_TOKEN_KEY)
        } else {
            self.store.set(auth::DEV_TOKEN_KEY, token)
        };
        if let Err(e) = saved {
            self.error = Some(format!("Couldn't keep the token: {e:#}"));
            return;
        }
        self.api = auth::saved(&self.store).map(|t| Api::new(api::base_url(), t));
        self.me = None;
        self.error = None;
        self.refresh();
    }

    /// Ask heard the allowance with an answer: keep the Account section in step without a fetch.
    pub fn note_questions_left(&mut self, left: u32) {
        if let Some(me) = &mut self.me {
            me.ask.questions_left = left;
        }
    }

    /// Picks up finished calls; returns messages for toasts.
    pub fn poll(&mut self, ctx: &egui::Context) -> Vec<String> {
        let mut toasts = Vec::new();
        while let Ok(done) = self.rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            match done {
                Done::SignedIn(attempt, _) if attempt != self.attempt => {}
                Done::SignedIn(_, r) => {
                    self.busy = None;
                    match r {
                        Ok(tokens) => {
                            self.api = Some(Api::new(api::base_url(), tokens));
                            self.refresh();
                        }
                        Err(e) => self.error = Some(format!("Couldn't sign in: {e}")),
                    }
                }
                Done::Me(Ok(me)) => {
                    self.me = Some(me);
                    self.error = None;
                }
                Done::Me(Err(ApiError::SignIn(_))) => {
                    // The session ended (and a refresh didn't bring it back).
                    self.api = None;
                    self.me = None;
                    self.error = Some("Your session ended. Sign in again.".into());
                }
                Done::Me(Err(e)) => self.error = Some(e.to_string()),
                Done::Opened(r) => {
                    self.busy = None;
                    match r {
                        Ok(url) => {
                            crate::app::open_url(&url);
                            self.awaiting_browser = true;
                        }
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
                Done::Deleted(Ok(())) => {
                    self.busy = None;
                    self.sign_out();
                    toasts.push("Your Linger account was deleted.".into());
                }
                Done::Deleted(Err(e)) => {
                    self.busy = None;
                    self.error = Some(format!("Couldn't delete the account: {e}"));
                }
                Done::SignedOut => {}
            }
        }
        // Back from the browser (checkout, the portal): the plan may have changed.
        let focused = ctx.input(|i| i.focused);
        if focused && !self.was_focused && self.awaiting_browser {
            self.awaiting_browser = false;
            self.refresh();
        }
        self.was_focused = focused;
        if self.pending > 0 {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        toasts
    }

    /// Screenshots: an account in a given state without a server.
    pub fn demo(&mut self, me: Option<Me>, busy: Option<&'static str>, error: Option<String>) {
        self.api = me.as_ref().map(|_| Api::new(api::base_url(), Arc::new(auth::StaticToken("demo".into()))));
        self.me = me;
        self.busy = busy;
        self.error = error;
    }
}

/// "November 1" from an ISO timestamp ("2026-11-01T00:00:00.000Z").
pub fn day(iso: &str) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let mut parts = iso.get(..10).unwrap_or("").split('-');
    let (_, m, d) = (parts.next(), parts.next(), parts.next());
    match (m.and_then(|m| m.parse::<usize>().ok()), d.and_then(|d| d.parse::<u32>().ok())) {
        (Some(m @ 1..=12), Some(d)) => format!("{} {d}", MONTHS[m - 1]),
        _ => iso.to_string(),
    }
}

/// "12.5 hours", "1 hour", "40 minutes".
pub fn hours(h: f64) -> String {
    if h < 1.0 {
        let m = (h * 60.0).round() as u32;
        return format!("{m} minute{}", if m == 1 { "" } else { "s" });
    }
    let r = (h * 10.0).round() / 10.0;
    if r == 1.0 {
        "1 hour".into()
    } else if r.fract() == 0.0 {
        format!("{r:.0} hours")
    } else {
        format!("{r:.1} hours")
    }
}
