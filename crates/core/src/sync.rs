//! The sync engine (docs/plans/library.md, "The sync engine"): keeps `study.sqlite`'s library
//! cache in step with Convex on its own `"sync"` thread.
//!
//! - **Pull:** a subscription to `library/sync:changes({ after })`. Each page is applied in one
//!   SQLite transaction that also moves the cursor; then the engine subscribes again from the
//!   page's `seq`. An empty page means caught up; `resync` empties the cache and pulls from 0.
//! - **Push:** the outbox, in order, each change carrying `baseSeq`. `ok` removes it; `conflict`
//!   removes it too (the server's copy stands and the cache takes it), recording the conflict so
//!   the UI can say so; a failure backs off. Nothing is sent until the feed has caught up once, so
//!   a device pulls the library before uploading what it has.
//! - **Tokens:** the backend asks [`TokenSource`] on every connect (`set_auth_callback`), and the
//!   engine installs it again 30 s before the token's `exp`, so a write never waits for Convex to
//!   refuse an expired token first (the M1 spike, docs/plans/backend.md).
//!
//! Everything a device asks of Convex goes through one [`Backend`]: [`ConvexBackend`] (feature
//! `convex`) or [`FakeBackend`] for tests. The engine sleeps in [`Backend::park`], so the Convex
//! backend's current-thread runtime runs its socket while the engine waits.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::auth::{self, TokenSource};
use crate::records::{Anchor, Changes, Chat, FeedRecord, Item, ItemKind, ItemStatus, VerseId, WriteAnswer};
use crate::study::cache;

/// The change feed's query.
pub const FEED: &str = "library/sync:changes";
/// The server's clock, from a mutation (never cached).
pub const CLOCK: &str = "library/sync:now";
/// Records an uploaded file.
const ATTACH: &str = "library/files:attach";
/// Install the token again this long before it expires.
const TOKEN_EARLY: Duration = Duration::from_secs(30);
/// The longest a failing change waits before it is tried again.
const MAX_BACKOFF_MS: i64 = 5 * 60 * 1000;

/// Why a call to the backend didn't succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The server couldn't be reached, or didn't answer in time: try again later.
    Offline(String),
    /// The server ran the function and refused (an error the user may need to see).
    Refused(String),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Offline(m) | CallError::Refused(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for CallError {}

/// Where a subscription's results go, each as JSON (or why it failed).
pub type Sink = Arc<dyn Fn(Result<Value, CallError>) + Send + Sync>;
/// Wakes the engine from [`Backend::park`]; callable from any thread.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Which kind of Convex function [`SyncHandle::call`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Query,
    Mutation,
    Action,
}

/// Every call a device makes to Convex. Arguments and results are JSON; numbers are integers
/// where the server's are whole.
pub trait Backend: Send {
    /// Authenticates with `tokens`, asked again on every reconnect. Called at start and again
    /// shortly before each token expires.
    fn set_auth(&mut self, tokens: Arc<dyn TokenSource>);
    /// Subscribes to the change feed from `after`, replacing the previous subscription. `sink`
    /// gets the first result and every later one.
    fn subscribe_changes(&mut self, after: u64, sink: Sink);
    fn mutate(&mut self, name: &str, args: Value) -> Result<Value, CallError>;
    fn query(&mut self, name: &str, args: Value) -> Result<Value, CallError>;
    fn action(&mut self, name: &str, args: Value) -> Result<Value, CallError>;
    /// A URL to upload one file's bytes to.
    fn upload_url(&mut self) -> Result<String, CallError>;
    /// Uploads bytes to an upload URL; returns the storage id.
    fn upload(&mut self, url: &str, bytes: Vec<u8>, content_type: &str) -> Result<String, CallError>;
    /// Downloads a file from its permanent URL.
    fn download(&mut self, url: &str) -> Result<Vec<u8>, CallError>;
    /// Whether the connection is up, as far as the backend knows. While it isn't, the engine
    /// holds the outbox and the footer says offline.
    fn connected(&self) -> bool {
        true
    }
    fn waker(&self) -> Waker;
    /// Sleeps until woken or `timeout`, running the backend's own work (its socket) meanwhile.
    fn park(&mut self, timeout: Duration);
}

/// What the sidebar footer says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    SignedOut,
    /// Everything here is on the server, and the feed is caught up.
    Synced,
    /// Changes waiting to be sent, or the feed still catching up.
    Syncing(usize),
    /// The server can't be reached; this many changes are waiting.
    Offline(usize),
    /// Something was refused; Retry tries again.
    Error(String),
}

#[derive(Debug)]
struct Shared {
    status: SyncStatus,
    cursor: u64,
    clock_offset_ms: i64,
}

struct Inner {
    state: Mutex<Shared>,
    changed: Condvar,
    on_change: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

enum Event {
    /// A result of the subscription made from `after`.
    Feed(u64, Result<Value, CallError>),
    Kick,
    Retry,
    Call(CallKind, String, Value, Sender<Result<Value, CallError>>),
    Stop,
}

/// The running engine, as the app holds it. Dropping it stops the thread.
pub struct SyncHandle {
    inner: Arc<Inner>,
    tx: Sender<Event>,
    waker: Waker,
    thread: Option<JoinHandle<()>>,
}

/// Starts the engine on a `"sync"` thread for `data_dir/study.sqlite`. The desktop calls this
/// after the first frame, so start-up never waits for it.
pub fn start(data_dir: &Path, backend: Box<dyn Backend>, tokens: Arc<dyn TokenSource>) -> Result<SyncHandle> {
    let conn = cache::open(&data_dir.join("study.sqlite"))?;
    let files_dir = data_dir.join("files");
    let inner = Arc::new(Inner {
        state: Mutex::new(Shared {
            status: SyncStatus::Syncing(cache::pending(&conn)?),
            cursor: cache::cursor(&conn)?,
            clock_offset_ms: cache::clock_offset_ms(&conn)?,
        }),
        changed: Condvar::new(),
        on_change: Mutex::new(None),
    });
    let (tx, rx) = channel();
    let waker = backend.waker();
    let mut engine = Engine {
        conn,
        backend,
        tokens,
        inner: inner.clone(),
        rx,
        tx: tx.clone(),
        waker: waker.clone(),
        subscribed: 0,
        caught_up: false,
        caught_up_once: false,
        written: 0,
        offline: None,
        offline_until: None,
        offline_attempts: 0,
        connected: true,
        feed_error: None,
        clock_known: false,
        held: None,
        token_due: None,
        data_changed: false,
        files_dir,
        fetch_due: true,
        unfetchable: HashSet::new(),
    };
    let thread = std::thread::Builder::new().name("sync".into()).spawn(move || engine.run())?;
    Ok(SyncHandle { inner, tx, waker, thread: Some(thread) })
}

impl SyncHandle {
    pub fn status(&self) -> SyncStatus {
        self.lock().status.clone()
    }

    /// The `seq` of the last feed page applied.
    pub fn cursor(&self) -> u64 {
        self.lock().cursor
    }

    /// The server's clock minus this device's, in milliseconds, from the last feed page.
    pub fn clock_offset_ms(&self) -> i64 {
        self.lock().clock_offset_ms
    }

    /// Calls `f` (on the sync thread) when the cache or the status changed: the desktop requests a
    /// repaint, and `StudyStore::refresh` picks the changes up.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.inner.on_change.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    }

    /// A function that tells the engine a change was queued: give it to `StudyStore::on_outbox`.
    pub fn notifier(&self) -> impl Fn() + Send + Sync + 'static {
        let (tx, waker) = (Mutex::new(self.tx.clone()), self.waker.clone());
        move || {
            let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(Event::Kick);
            waker();
        }
    }

    /// Tries every waiting change again now (the footer's Retry).
    pub fn retry(&self) {
        self.send(Event::Retry);
    }

    /// Runs a Convex function through the engine's connection, blocking until it answers (the
    /// Ask tool loop, from M2).
    pub fn call(&self, kind: CallKind, name: &str, args: Value) -> Result<Value, CallError> {
        let (reply, answer) = channel();
        self.send(Event::Call(kind, name.into(), args, reply));
        answer.recv().unwrap_or_else(|_| Err(CallError::Offline("sync stopped".into())))
    }

    /// Waits until the outbox is empty and the feed is caught up; returns the cursor, or the
    /// status at `timeout`, or at once on an error, which waiting doesn't mend (a refusal, or a
    /// sign-in that ended).
    pub fn wait_idle(&self, timeout: Duration) -> Result<u64, SyncStatus> {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            match &state.status {
                SyncStatus::Synced => return Ok(state.cursor),
                SyncStatus::Error(_) => return Err(state.status.clone()),
                _ => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(state.status.clone());
            }
            state = self.inner.changed.wait_timeout(state, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    fn send(&self, e: Event) {
        let _ = self.tx.send(e);
        (self.waker)();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for SyncHandle {
    fn drop(&mut self) {
        self.send(Event::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 1 s, doubling with each attempt, up to 5 minutes.
fn backoff(attempts: i64) -> Duration {
    Duration::from_millis((1000i64 << (attempts - 1).clamp(0, 20)).min(MAX_BACKOFF_MS) as u64)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

struct Engine {
    conn: Connection,
    backend: Box<dyn Backend>,
    tokens: Arc<dyn TokenSource>,
    inner: Arc<Inner>,
    rx: Receiver<Event>,
    tx: Sender<Event>,
    waker: Waker,
    /// The cursor the current subscription started from.
    subscribed: u64,
    /// The last page was empty: nothing newer on the server.
    caught_up: bool,
    caught_up_once: bool,
    /// The highest `seq` the server gave this device's own writes: caught up means the cursor
    /// has reached it too.
    written: u64,
    offline: Option<String>,
    /// While offline, no change is tried before this; it backs off with each failed attempt.
    offline_until: Option<Instant>,
    offline_attempts: u32,
    connected: bool,
    feed_error: Option<String>,
    /// The clock offset was measured on this connection; changes wait for it, since they carry
    /// creation times moved into server time.
    clock_known: bool,
    /// A feed page that arrived before the clock was measured, held until it is: pages carry
    /// creation times, and the stored offset may be another connection's, or none.
    held: Option<(u64, Value)>,
    token_due: Option<Instant>,
    /// The cache changed since the last `on_change`.
    data_changed: bool,
    /// Where fetched text files go.
    files_dir: PathBuf,
    /// The cache may want files it doesn't have.
    fetch_due: bool,
    /// File URLs the server refused (the file was replaced or deleted since): not asked for again
    /// until restart. Keyed by URL, not by owner, so a chat's next conversation is still fetched.
    unfetchable: HashSet<String>,
}

impl Engine {
    fn run(&mut self) {
        self.authenticate();
        // The clock first, so the first pages a new device pulls are dated by it too; offline,
        // the loop measures it once the server answers.
        self.measure_clock();
        let after = cache::cursor(&self.conn).unwrap_or(0);
        self.subscribe(after);
        loop {
            while let Ok(event) = self.rx.try_recv() {
                if !self.handle(event) {
                    return;
                }
            }
            let connected = self.backend.connected();
            if connected && !self.connected {
                // Back online: what was waiting goes at once, and a feed that ended offline resumes.
                self.online();
                self.clock_known = false;
                if !self.caught_up {
                    let after = self.subscribed;
                    self.subscribe(after);
                }
            }
            self.connected = connected;
            if !connected {
                self.offline.get_or_insert_with(|| "Can't reach Linger's server".into());
            } else {
                if !self.clock_known && self.feed_error.is_none() {
                    self.measure_clock();
                }
                if self.clock_known {
                    if let Some((after, value)) = self.held.take()
                        && after == self.subscribed
                        && let Err(e) = self.apply(value)
                    {
                        self.feed_error = Some(format!("{e:#}"));
                    }
                    if self.caught_up_once {
                        self.drain();
                    }
                    self.fetch();
                }
            }
            if self.token_due.is_some_and(|due| Instant::now() >= due) {
                self.authenticate();
            }
            self.publish();
            self.backend.park(self.timeout());
        }
    }

    fn authenticate(&mut self) {
        self.backend.set_auth(self.tokens.clone());
        self.token_due = match self.tokens.token() {
            Ok(t) => auth::expires_at(&t).map(|exp| {
                let left = Duration::from_secs(exp).saturating_sub(
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default() + TOKEN_EARLY,
                );
                // A token already inside its last 30 s: try again shortly, not in a tight loop.
                Instant::now() + left.max(Duration::from_secs(5))
            }),
            Err(e) => {
                self.feed_error = Some(format!("Sign in again: {e:#}"));
                None
            }
        };
    }

    fn subscribe(&mut self, after: u64) {
        self.subscribed = after;
        self.caught_up = false;
        let (tx, waker) = (Mutex::new(self.tx.clone()), self.waker.clone());
        self.backend.subscribe_changes(
            after,
            Arc::new(move |r| {
                let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(Event::Feed(after, r));
                waker();
            }),
        );
    }

    /// Handles one event; `false` to stop.
    fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::Feed(after, _) if after != self.subscribed => {}
            Event::Feed(after, Ok(value)) => {
                self.online();
                if !self.clock_known {
                    // The latest page replaces any held one: it is the subscription's whole answer.
                    self.held = Some((after, value));
                } else if let Err(e) = self.apply(value) {
                    self.feed_error = Some(format!("{e:#}"));
                }
            }
            Event::Feed(_, Err(CallError::Offline(m))) => self.offline = Some(m),
            Event::Feed(_, Err(CallError::Refused(m))) => self.feed_error = Some(m),
            Event::Kick => {}
            Event::Retry => {
                let _ = cache::retry_all(&self.conn);
                self.feed_error = None;
                self.online();
                if !self.caught_up {
                    let after = self.subscribed;
                    self.subscribe(after);
                }
            }
            Event::Call(kind, name, args, reply) => {
                let r = match kind {
                    CallKind::Query => self.backend.query(&name, args),
                    CallKind::Mutation => self.backend.mutate(&name, args),
                    CallKind::Action => self.backend.action(&name, args),
                };
                let _ = reply.send(r);
            }
            Event::Stop => return false,
        }
        true
    }

    /// Applies one result of the feed.
    fn apply(&mut self, value: Value) -> Result<()> {
        // A record type newer than this build: stop here rather than skip it (the cursor would
        // move past records this device never kept), and say what mends it.
        let known = ["highlight", "bookmark", "item", "chat"];
        if let Some(t) = value["records"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|r| r["type"].as_str().filter(|t| !known.contains(t)))
        {
            anyhow::bail!(
                "Update Linger to keep syncing: your library has {t} records this version can't read."
            );
        }
        match serde_json::from_value::<Changes>(value)? {
            Changes::Page { records, seq, .. } => {
                if cache::apply_page(&mut self.conn, &records, seq)? > 0 {
                    self.data_changed = true;
                    self.fetch_due = true;
                }
                self.feed_error = None;
                if records.is_empty() {
                    self.caught_up = true;
                    self.caught_up_once = true;
                } else {
                    self.subscribe(seq);
                }
            }
            Changes::Resync { .. } => {
                cache::resync(&mut self.conn)?;
                self.data_changed = true;
                self.subscribe(0);
            }
        }
        Ok(())
    }

    /// Measures the device's clock against the server's with a mutation, which Convex never
    /// serves from a cache (a query's `Clock` is the time of its last execution, so the feed's
    /// `serverTime` is not a clock), corrected by half the round trip.
    fn measure_clock(&mut self) {
        if self.offline_until.is_some_and(|t| Instant::now() < t) {
            return;
        }
        let sent = now_ms();
        match self.backend.mutate(CLOCK, json!({})) {
            Ok(answer) => {
                let back = now_ms();
                let Some(server) = answer["serverTime"].as_i64() else {
                    self.feed_error = Some(format!("unexpected answer from {CLOCK}: {answer}"));
                    return;
                };
                self.online();
                if cache::set_clock_offset_ms(&mut self.conn, server - (sent + back) / 2).is_ok() {
                    self.clock_known = true;
                }
            }
            Err(CallError::Offline(m)) => {
                self.offline = Some(m);
                self.offline_attempts += 1;
                self.offline_until = Some(Instant::now() + backoff(self.offline_attempts as i64));
            }
            Err(CallError::Refused(m)) => self.feed_error = Some(m),
        }
    }

    /// The server answered: changes can go again at once.
    fn online(&mut self) {
        self.offline = None;
        self.offline_until = None;
        self.offline_attempts = 0;
    }

    /// Sends waiting changes, oldest first, until one isn't due or the server can't be reached.
    fn drain(&mut self) {
        while self.offline_until.is_none_or(|t| Instant::now() >= t) {
            let entry = match cache::next(&self.conn) {
                Ok(Some(e)) if e.next_at <= now_ms() => e,
                Ok(_) => return,
                Err(e) => {
                    self.feed_error = Some(format!("{e:#}"));
                    return;
                }
            };
            let done = match self.send(&entry) {
                Ok(answer) => {
                    self.online();
                    match &entry.key {
                        cache::Key::Uid(_) => match serde_json::from_value::<WriteAnswer>(answer.clone()) {
                            Ok(answer) => self.settle(&entry, answer),
                            Err(_) => self.refused(&entry, &format!("unexpected answer {answer}")),
                        },
                        cache::Key::Verse(_) => {
                            let seq = answer["seq"].as_u64().unwrap_or(0);
                            self.written = self.written.max(seq);
                            match answer["result"].as_str() {
                                Some("ok") => cache::sent(&mut self.conn, &entry, seq, None),
                                Some("conflict") => {
                                    self.data_changed = true;
                                    cache::conflicted(&mut self.conn, &entry, &answer).map(|_| ())
                                }
                                _ => self.refused(&entry, &format!("unexpected answer {answer}")),
                            }
                        }
                    }
                }
                Err(CallError::Offline(m)) => {
                    self.offline = Some(m);
                    self.offline_attempts += 1;
                    self.offline_until = Some(Instant::now() + backoff(self.offline_attempts as i64));
                    return;
                }
                Err(CallError::Refused(m)) => self.refused(&entry, &m),
            };
            if let Err(e) = done {
                self.feed_error = Some(format!("{e:#}"));
                return;
            }
        }
    }

    /// Takes the server's answer to a note's or chat's write: the records it holds, as the feed
    /// will send them, become this device's.
    fn settle(&mut self, entry: &cache::Entry, answer: WriteAnswer) -> Result<()> {
        match answer {
            WriteAnswer::Ok { seq, record } => {
                self.written = self.written.max(seq);
                self.data_changed = true;
                cache::sent(&mut self.conn, entry, seq, record.as_ref())
            }
            WriteAnswer::Conflict { seq, record, copy } => {
                // A note's conflict copy is this device's write too.
                self.written = self.written.max(seq).max(copy.as_ref().map_or(0, FeedRecord::seq));
                self.data_changed = true;
                self.fetch_due = true;
                cache::conflicted_by_uid(&mut self.conn, entry, &record, copy.as_ref()).map(|_| ())
            }
        }
    }

    /// Sends one change: its file first, if it has one (an upload URL, the bytes, then
    /// `files.attach`), then its mutation.
    fn send(&mut self, entry: &cache::Entry) -> Result<Value, CallError> {
        if let (Some((file_uid, bytes, sha256)), cache::Key::Uid(owner)) = (&entry.upload, &entry.key) {
            let url = self.backend.upload_url()?;
            let storage_id = self.backend.upload(&url, bytes.clone(), "application/json")?;
            let file = self.backend.mutate(
                ATTACH,
                json!({ "uid": file_uid, "ownerUid": owner, "role": "chat", "storageId": storage_id, "sha256": sha256 }),
            )?;
            cache::attached(&self.conn, owner, file_uid, file["url"].as_str().unwrap_or_default())
                .map_err(|e| CallError::Refused(format!("{e:#}")))?;
        }
        self.backend.mutate(entry.name, entry.args.clone())
    }

    /// Fetches the files the cache wants (conversations and text), while the server answers.
    fn fetch(&mut self) {
        if !self.fetch_due {
            return;
        }
        self.fetch_due = false;
        for f in cache::to_fetch(&self.conn).unwrap_or_default() {
            if self.unfetchable.contains(&f.url) {
                continue;
            }
            match self.backend.download(&f.url) {
                Ok(bytes) => match cache::fetched(&self.conn, &f, &bytes, &self.files_dir) {
                    Ok(()) => self.data_changed = true,
                    Err(_) => {
                        self.unfetchable.insert(f.url);
                    }
                },
                Err(CallError::Offline(_)) => {
                    // Tried again when the loop next comes round.
                    self.fetch_due = true;
                    return;
                }
                Err(CallError::Refused(_)) => {
                    self.unfetchable.insert(f.url);
                }
            }
        }
    }

    /// Files the cache still wants and can get.
    fn fetching(&self) -> bool {
        cache::to_fetch(&self.conn).unwrap_or_default().iter().any(|f| !self.unfetchable.contains(&f.url))
    }

    fn refused(&self, entry: &cache::Entry, error: &str) -> Result<()> {
        let at = now_ms() + backoff(entry.attempts + 1).as_millis() as i64;
        cache::failed(&self.conn, entry, error, at)
    }

    fn timeout(&self) -> Duration {
        let mut t = Duration::from_secs(30);
        if let Ok(Some(at)) = cache::wake_at(&self.conn) {
            t = t.min(Duration::from_millis((at - now_ms()).max(0) as u64));
        }
        if let Some(until) = self.offline_until {
            t = t.min(until.saturating_duration_since(Instant::now()));
        }
        if let Some(due) = self.token_due {
            t = t.min(due.saturating_duration_since(Instant::now()));
        }
        t
    }

    /// Tells the app what changed.
    fn publish(&mut self) {
        let pending = cache::pending(&self.conn).unwrap_or(0);
        let status = if self.offline.is_some() {
            SyncStatus::Offline(pending)
        } else if let Some(e) =
            self.feed_error.clone().or_else(|| cache::last_error(&self.conn).ok().flatten())
        {
            SyncStatus::Error(e)
        } else if pending > 0
            || !self.caught_up
            || cache::cursor(&self.conn).unwrap_or(0) < self.written
            || self.fetching()
        {
            SyncStatus::Syncing(pending)
        } else {
            SyncStatus::Synced
        };
        let changed = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.cursor = cache::cursor(&self.conn).unwrap_or(state.cursor);
            state.clock_offset_ms = cache::clock_offset_ms(&self.conn).unwrap_or(state.clock_offset_ms);
            let changed = state.status != status || self.data_changed;
            state.status = status;
            changed
        };
        self.inner.changed.notify_all();
        if changed {
            self.data_changed = false;
            if let Some(f) = &*self.inner.on_change.lock().unwrap_or_else(|e| e.into_inner()) {
                f();
            }
        }
    }
}

/// An in-memory Convex for tests: the library's per-verse writes with the server's conflict rule,
/// the change feed (paged, with tombstones and `resync`), and failure injection. Several
/// [`FakeBackend`]s on one server are several devices of one user.
#[derive(Default)]
pub struct FakeServer {
    state: Mutex<FakeState>,
}

#[derive(Default)]
struct FakeState {
    seq: u64,
    purged_through: u64,
    /// By kind ("highlight", "bookmark"), then verse: (seq, value, deleted).
    records: BTreeMap<&'static str, BTreeMap<u32, (u64, String, bool)>>,
    /// When each record was made, by kind and verse, in milliseconds.
    created: BTreeMap<(&'static str, u32), u64>,
    subscribers: Vec<(u64, u64, Sink)>,
    next_backend: u64,
    offline: bool,
    failures: VecDeque<CallError>,
    page_records: Option<usize>,
    /// When the user last wrote: like Convex, the feed is a cached query, so its `serverTime` is
    /// the time it last ran, which a write causes.
    last_write_ms: i64,
    uploads: BTreeMap<String, Vec<u8>>,
    /// Notes and chats by uid, as the feed sends them.
    items: BTreeMap<String, Item>,
    chats: BTreeMap<String, Chat>,
    /// Attached files by uid: (owner uid, role, storage id).
    files: BTreeMap<String, (Option<String>, String, String)>,
}

/// What `convex/lib/conflict.ts`'s `decideByUid` decides.
enum ByUid {
    Write,
    Unchanged(u64),
    Conflict(u64),
}

/// `decideByUid`: `stored` is (seq, deleted, already holds what is written).
fn decide_by_uid(stored: Option<(u64, bool, bool)>, removing: bool, base: Option<u64>) -> ByUid {
    match stored {
        None if removing => ByUid::Unchanged(0),
        None => ByUid::Write,
        Some((seq, deleted, same)) if (removing && deleted) || (!removing && same && !deleted) => {
            ByUid::Unchanged(seq)
        }
        Some((seq, ..)) if base.is_some_and(|b| seq > b) => ByUid::Conflict(seq),
        Some(_) => ByUid::Write,
    }
}

fn refused(e: impl fmt::Display) -> CallError {
    CallError::Refused(e.to_string())
}

impl FakeServer {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// Another device's connection.
    pub fn backend(self: &Arc<Self>) -> FakeBackend {
        let mut s = self.lock();
        s.next_backend += 1;
        FakeBackend { server: self.clone(), id: s.next_backend, wake: Arc::default() }
    }

    /// While offline, calls fail with [`CallError::Offline`] and the feed is silent; coming back
    /// delivers every subscription's current page.
    pub fn set_offline(&self, offline: bool) {
        let mut s = self.lock();
        s.offline = offline;
        s.deliver_all();
    }

    /// The next mutation fails with `e`.
    pub fn fail_next(&self, e: CallError) {
        self.lock().failures.push_back(e);
    }

    /// Pages hold at most `n` records (the real feed holds 500).
    pub fn set_page_records(&self, n: usize) {
        self.lock().page_records = Some(n);
    }

    /// The purge chain: drops every tombstone, so cursors behind them must resync.
    pub fn purge_tombstones(&self) {
        let mut s = self.lock();
        s.purged_through = s.seq;
        for records in s.records.values_mut() {
            records.retain(|_, r| !r.2);
        }
        s.items.retain(|_, i| i.deleted_at.is_none());
        s.chats.retain(|_, c| c.deleted_at.is_none());
        s.deliver_all();
    }

    /// The scripted feed: delivers `result` to every subscription as is.
    pub fn deliver(&self, result: Value) {
        let s = self.lock();
        for (_, _, sink) in &s.subscribers {
            sink(Ok(result.clone()));
        }
    }

    /// The server's highlight for a verse: (seq, colour, deleted).
    pub fn highlight(&self, verse: VerseId) -> Option<(u64, String, bool)> {
        self.lock().records.get("highlight")?.get(&verse.0).cloned()
    }

    pub fn bookmark(&self, verse: VerseId) -> Option<(u64, String, bool)> {
        self.lock().records.get("bookmark")?.get(&verse.0).cloned()
    }

    /// The live notes, conflict copies included.
    pub fn notes(&self) -> Vec<Item> {
        self.lock().items.values().filter(|i| i.deleted_at.is_none()).cloned().collect()
    }

    /// Stores an item as another device would have made it (a sermon or a captured page, which
    /// this build can't make yet), at the next `seq`, and delivers it.
    pub fn add_item(&self, mut item: Item) {
        let mut s = self.lock();
        item.seq = s.next_seq();
        item.updated_at = Some(now_ms() as u64);
        s.items.insert(item.uid.clone(), item);
        s.wrote();
    }

    /// The live chats.
    pub fn chats(&self) -> Vec<Chat> {
        self.lock().chats.values().filter(|c| c.deleted_at.is_none()).cloned().collect()
    }

    /// The user's latest `seq`.
    pub fn seq(&self) -> u64 {
        self.lock().seq
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl FakeState {
    fn page(&self, after: u64) -> Value {
        if after > 0 && after < self.purged_through {
            return json!({ "resync": true });
        }
        let mut records: Vec<Value> = self
            .records
            .iter()
            .flat_map(|(kind, rs)| {
                rs.iter().filter(|(_, r)| r.0 > after).map(move |(verse, (seq, value, deleted))| {
                    let field = if *kind == "highlight" { "color" } else { "label" };
                    let mut r = json!({
                        "type": kind, "uid": format!("{}-{verse}", &kind[..1]), "seq": seq, "verse": verse,
                        field: value,
                    });
                    if *deleted {
                        r["deletedAt"] = json!(1);
                    }
                    if let Some(created) = self.created.get(&(*kind, *verse)) {
                        r["createdAt"] = json!(created);
                    }
                    r
                })
            })
            .collect();
        let by_uid =
            self.items.values().filter(|i| i.seq > after).map(|i| FeedRecord::Item(Box::new(i.clone())));
        let by_uid =
            by_uid.chain(self.chats.values().filter(|c| c.seq > after).map(|c| FeedRecord::Chat(c.clone())));
        records.extend(by_uid.map(|r| serde_json::to_value(r).unwrap_or_default()));
        records.sort_by_key(|r| r["seq"].as_u64());
        records.truncate(self.page_records.unwrap_or(500));
        let seq = records.last().map_or(after, |r| r["seq"].as_u64().unwrap_or(after));
        json!({ "records": records, "seq": seq, "serverTime": self.last_write_ms })
    }

    fn deliver_all(&self) {
        if self.offline {
            return;
        }
        for (_, after, sink) in &self.subscribers {
            sink(Ok(self.page(*after)));
        }
    }

    /// `highlights.set` and `bookmarks.set`, with `convex/lib/conflict.ts`'s rule.
    fn set(&mut self, kind: &'static str, field: &str, args: &Value) -> Result<Value, CallError> {
        let verse = args["verse"].as_u64().and_then(|v| u32::try_from(v).ok()).map(VerseId);
        let Some(verse) = verse.filter(|v| v.is_valid()) else {
            return Err(CallError::Refused(format!("not a verse: {}", args["verse"])));
        };
        let value = args[field].as_str().map(str::to_string);
        let base = args["baseSeq"].as_u64();
        let stored = self.records.get(kind).and_then(|rs| rs.get(&verse.0)).cloned();
        if let (Some((seq, v, deleted)), Some(base)) = (&stored, base)
            && *seq > base
        {
            let created = self.created.get(&(kind, verse.0));
            return Ok(
                json!({ "result": "conflict", "seq": seq, field: (!deleted).then_some(v), "createdAt": created }),
            );
        }
        if value.is_none() && stored.as_ref().is_none_or(|s| s.2) {
            return Ok(json!({ "result": "ok", "seq": stored.map_or(0, |s| s.0) }));
        }
        self.seq += 1;
        let seq = self.seq;
        if value.is_some() && stored.as_ref().is_none_or(|s| s.2) {
            let created = args["createdAt"].as_u64().unwrap_or_else(|| now_ms() as u64);
            self.created.insert((kind, verse.0), created);
        }
        let record = match value {
            Some(v) => (seq, v, false),
            None => (seq, stored.map(|s| s.1).unwrap_or_default(), true),
        };
        self.records.entry(kind).or_default().insert(verse.0, record);
        self.wrote();
        Ok(json!({ "result": "ok", "seq": seq }))
    }

    fn wrote(&mut self) {
        self.last_write_ms = now_ms();
        self.deliver_all();
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// `notes.upsert`: a stale edit is kept as a conflict copy.
    fn upsert_note(&mut self, args: &Value) -> Result<Value, CallError> {
        let uid = args["uid"].as_str().ok_or_else(|| refused("no uid"))?.to_string();
        let text = args["text"].as_str().unwrap_or_default().to_string();
        let anchors: Vec<Anchor> = serde_json::from_value(args["anchors"].clone()).map_err(refused)?;
        if anchors.iter().any(|a| !a.from.is_valid() || !a.to.is_valid() || a.from > a.to) {
            return Err(refused("That reference isn't in the Bible."));
        }
        let (base, captured) = (args["baseSeq"].as_u64(), args["capturedAt"].as_u64());
        let stored = self.items.get(&uid).map(|i| {
            (i.seq, i.deleted_at.is_some(), i.text.as_deref() == Some(text.as_str()) && i.anchors == anchors)
        });
        Ok(answer(match decide_by_uid(stored, false, base) {
            ByUid::Unchanged(seq) => WriteAnswer::Ok { seq, record: self.item(&uid) },
            ByUid::Write => {
                let seq = self.write_note(&uid, &text, anchors, captured, None);
                WriteAnswer::Ok { seq, record: self.item(&uid) }
            }
            ByUid::Conflict(seq) => {
                let copy = format!("{uid}-c{}", base.unwrap_or(0));
                if !self.items.contains_key(&copy) {
                    self.write_note(&copy, &text, anchors, captured, Some(&uid));
                }
                WriteAnswer::Conflict {
                    seq,
                    record: self.item(&uid).expect("stored"),
                    copy: self.item(&copy),
                }
            }
        }))
    }

    fn write_note(
        &mut self,
        uid: &str,
        text: &str,
        anchors: Vec<Anchor>,
        captured: Option<u64>,
        conflict_of: Option<&str>,
    ) -> u64 {
        let seq = self.next_seq();
        let now = now_ms() as u64;
        let title =
            text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(80).collect();
        let item = match self.items.remove(uid) {
            Some(i) => Item {
                seq,
                deleted_at: None,
                captured_at: if i.deleted_at.is_some() { captured.unwrap_or(now) } else { i.captured_at },
                title,
                text: Some(text.into()),
                anchors,
                updated_at: Some(now),
                ..i
            },
            None => Item {
                uid: uid.into(),
                seq,
                deleted_at: None,
                kind: ItemKind::Note,
                title,
                status: ItemStatus::Ready,
                text: Some(text.into()),
                text_file_uid: None,
                excerpt: None,
                summary: None,
                outline: None,
                source_uid: None,
                sermon_uid: None,
                offset_ms: None,
                recorded_on: None,
                details: None,
                conflict_of: conflict_of.map(Into::into),
                captured_at: captured.unwrap_or(now),
                updated_at: Some(now),
                unreviewed: false,
                anchors,
                files: Vec::new(),
            },
        };
        self.items.insert(uid.into(), item);
        self.wrote();
        seq
    }

    /// `items.updateDetails`: a field given replaces the stored one (`null` clears it).
    fn update_details(&mut self, args: &Value) -> Result<Value, CallError> {
        let uid = args["uid"].as_str().ok_or_else(|| refused("no uid"))?.to_string();
        let Some(stored) = self.items.get(&uid).filter(|i| i.deleted_at.is_none()).cloned() else {
            return Err(refused("That item isn't in your library."));
        };
        if let Some(sermon) = args["sermonUid"].as_str()
            && !self
                .items
                .get(sermon)
                .is_some_and(|s| s.kind == ItemKind::Sermon && s.deleted_at.is_none() && s.uid != uid)
        {
            return Err(refused("That sermon isn't in your library."));
        }
        let mut item = stored.clone();
        if let Some(t) = args["title"].as_str() {
            item.title = t.into();
        }
        if args.get("details").is_some() {
            item.details = serde_json::from_value(args["details"].clone()).map_err(refused)?;
        }
        if args.get("sermonUid").is_some() {
            item.sermon_uid = args["sermonUid"].as_str().map(Into::into);
        }
        if args.get("offsetMs").is_some() {
            item.offset_ms = args["offsetMs"].as_u64();
        }
        let same = item == stored;
        Ok(answer(match decide_by_uid(Some((stored.seq, false, same)), false, args["baseSeq"].as_u64()) {
            ByUid::Unchanged(seq) => WriteAnswer::Ok { seq, record: self.item(&uid) },
            ByUid::Conflict(seq) => {
                WriteAnswer::Conflict { seq, record: self.item(&uid).expect("stored"), copy: None }
            }
            ByUid::Write => {
                item.seq = self.next_seq();
                item.updated_at = Some(now_ms() as u64);
                let seq = item.seq;
                self.items.insert(uid.clone(), item);
                self.wrote();
                WriteAnswer::Ok { seq, record: self.item(&uid) }
            }
        }))
    }

    /// The item `uid` as the feed sends it.
    fn item(&self, uid: &str) -> Option<FeedRecord> {
        self.items.get(uid).map(|i| FeedRecord::Item(Box::new(i.clone())))
    }

    /// The chat `uid` as the feed sends it.
    fn chat(&self, uid: &str) -> Option<FeedRecord> {
        self.chats.get(uid).map(|c| FeedRecord::Chat(c.clone()))
    }

    fn remove_note(&mut self, args: &Value) -> Result<Value, CallError> {
        let uid = args["uid"].as_str().ok_or_else(|| refused("no uid"))?.to_string();
        let stored = self.items.get(&uid).map(|i| (i.seq, i.deleted_at.is_some(), false));
        Ok(answer(match decide_by_uid(stored, true, args["baseSeq"].as_u64()) {
            ByUid::Unchanged(seq) => WriteAnswer::Ok { seq, record: self.item(&uid) },
            ByUid::Conflict(seq) => {
                WriteAnswer::Conflict { seq, record: self.item(&uid).expect("stored"), copy: None }
            }
            ByUid::Write => {
                let seq = self.next_seq();
                let i = self.items.get_mut(&uid).expect("stored");
                (i.seq, i.deleted_at, i.updated_at, i.text, i.title, i.anchors) = (
                    seq,
                    Some(now_ms() as u64),
                    Some(now_ms() as u64),
                    Some(String::new()),
                    String::new(),
                    Vec::new(),
                );
                self.wrote();
                WriteAnswer::Ok { seq, record: self.item(&uid) }
            }
        }))
    }

    /// `files.attach`, idempotent by uid, checking the bytes' SHA-256.
    fn attach(&mut self, args: &Value) -> Result<Value, CallError> {
        use sha2::{Digest, Sha256};
        let uid = args["uid"].as_str().ok_or_else(|| refused("no uid"))?.to_string();
        let answer = |s: &FakeState, storage: &str| json!({ "uid": uid, "url": format!("fake://storage/{storage}"), "bytes": s.uploads.get(storage).map_or(0, Vec::len) });
        if let Some((_, _, storage)) = self.files.get(&uid) {
            return Ok(answer(self, storage));
        }
        let storage = args["storageId"].as_str().unwrap_or_default().to_string();
        let bytes = self.uploads.get(&storage).ok_or_else(|| refused("No upload has that storage id."))?;
        if format!("{:x}", Sha256::digest(bytes)) != args["sha256"].as_str().unwrap_or_default() {
            return Err(refused("The upload's bytes don't match its SHA-256."));
        }
        let owner = args["ownerUid"].as_str().map(Into::into);
        let role = args["role"].as_str().unwrap_or_default().into();
        self.files.insert(uid.clone(), (owner, role, storage.clone()));
        Ok(answer(self, &storage))
    }

    /// `chats.save`: the conversation must be attached first as the chat's own `chat` file.
    fn save_chat(&mut self, args: &Value) -> Result<Value, CallError> {
        let uid = args["uid"].as_str().ok_or_else(|| refused("no uid"))?.to_string();
        let (title, file_uid) = (
            args["title"].as_str().unwrap_or_default().to_string(),
            args["fileUid"].as_str().unwrap_or_default(),
        );
        let storage = match self.files.get(file_uid) {
            Some((Some(owner), role, storage)) if *owner == uid && role == "chat" => storage.clone(),
            _ => return Err(refused("Attach the conversation as this chat's file first.")),
        };
        let stored = self.chats.get(&uid).cloned();
        let same = stored
            .as_ref()
            .map(|c| (c.seq, c.deleted_at.is_some(), c.file_uid == file_uid && c.title == title));
        Ok(answer(match decide_by_uid(same, false, args["baseSeq"].as_u64()) {
            ByUid::Unchanged(seq) => WriteAnswer::Ok { seq, record: self.chat(&uid) },
            ByUid::Conflict(seq) => {
                let stored = stored.expect("stored");
                if stored.file_uid != file_uid {
                    self.files.remove(file_uid);
                }
                WriteAnswer::Conflict { seq, record: FeedRecord::Chat(stored), copy: None }
            }
            ByUid::Write => {
                let seq = self.next_seq();
                let created_at = match &stored {
                    Some(c) if c.deleted_at.is_none() => c.created_at,
                    _ => args["createdAt"].as_u64().unwrap_or(now_ms() as u64),
                };
                if let Some(old) = stored.filter(|c| c.file_uid != file_uid) {
                    self.files.remove(&old.file_uid);
                }
                let bytes = self.uploads.get(&storage).map_or(0, Vec::len) as u64;
                let chat = Chat {
                    uid: uid.clone(),
                    seq,
                    deleted_at: None,
                    created_at,
                    updated_at: Some(now_ms() as u64),
                    title,
                    file_uid: file_uid.into(),
                    url: format!("fake://storage/{storage}"),
                    bytes,
                };
                self.chats.insert(uid.clone(), chat);
                self.wrote();
                WriteAnswer::Ok { seq, record: self.chat(&uid) }
            }
        }))
    }

    fn remove_chat(&mut self, args: &Value) -> Result<Value, CallError> {
        let uid = args["uid"].as_str().ok_or_else(|| refused("no uid"))?.to_string();
        let stored = self.chats.get(&uid).map(|c| (c.seq, c.deleted_at.is_some(), false));
        Ok(answer(match decide_by_uid(stored, true, args["baseSeq"].as_u64()) {
            ByUid::Unchanged(seq) => WriteAnswer::Ok { seq, record: self.chat(&uid) },
            ByUid::Conflict(seq) => {
                WriteAnswer::Conflict { seq, record: self.chat(&uid).expect("stored"), copy: None }
            }
            ByUid::Write => {
                let seq = self.next_seq();
                let c = self.chats.get_mut(&uid).expect("stored");
                (c.seq, c.deleted_at, c.updated_at, c.title, c.url, c.bytes) =
                    (seq, Some(now_ms() as u64), Some(now_ms() as u64), String::new(), String::new(), 0);
                let file = c.file_uid.clone();
                self.files.remove(&file);
                self.wrote();
                WriteAnswer::Ok { seq, record: self.chat(&uid) }
            }
        }))
    }
}

fn answer(a: WriteAnswer) -> Value {
    serde_json::to_value(a).expect("an answer serializes")
}

/// One device's connection to a [`FakeServer`].
pub struct FakeBackend {
    server: Arc<FakeServer>,
    id: u64,
    wake: Arc<(Mutex<bool>, Condvar)>,
}

impl FakeBackend {
    fn call(&mut self, name: &str, args: &Value) -> Result<Value, CallError> {
        let mut s = self.server.lock();
        if s.offline {
            return Err(CallError::Offline("the fake server is offline".into()));
        }
        if let Some(e) = s.failures.pop_front() {
            return Err(e);
        }
        match name {
            "library/highlights:set" => s.set("highlight", "color", args),
            "library/bookmarks:set" => s.set("bookmark", "label", args),
            "library/notes:upsert" => s.upsert_note(args),
            "library/notes:remove" => s.remove_note(args),
            "library/items:updateDetails" => s.update_details(args),
            "library/chats:save" => s.save_chat(args),
            "library/chats:remove" => s.remove_chat(args),
            ATTACH => s.attach(args),
            FEED => Ok(s.page(args["after"].as_u64().unwrap_or(0))),
            CLOCK => Ok(json!({ "serverTime": now_ms() })),
            "library/files:uploadUrl" => Ok(json!(format!("fake://upload/{}", s.uploads.len()))),
            _ => Err(CallError::Refused(format!("no function {name}"))),
        }
    }
}

impl Backend for FakeBackend {
    fn set_auth(&mut self, _tokens: Arc<dyn TokenSource>) {}

    fn subscribe_changes(&mut self, after: u64, sink: Sink) {
        let mut s = self.server.lock();
        s.subscribers.retain(|(id, _, _)| *id != self.id);
        if !s.offline {
            sink(Ok(s.page(after)));
        }
        s.subscribers.push((self.id, after, sink));
    }

    fn mutate(&mut self, name: &str, args: Value) -> Result<Value, CallError> {
        self.call(name, &args)
    }

    fn query(&mut self, name: &str, args: Value) -> Result<Value, CallError> {
        self.call(name, &args)
    }

    fn action(&mut self, name: &str, args: Value) -> Result<Value, CallError> {
        self.call(name, &args)
    }

    fn upload_url(&mut self) -> Result<String, CallError> {
        Ok(self.call("library/files:uploadUrl", &json!({}))?.as_str().unwrap_or_default().to_string())
    }

    fn upload(&mut self, url: &str, bytes: Vec<u8>, _content_type: &str) -> Result<String, CallError> {
        let mut s = self.server.lock();
        let id = format!("storage-{}", url.rsplit('/').next().unwrap_or_default());
        s.uploads.insert(id.clone(), bytes);
        Ok(id)
    }

    fn download(&mut self, url: &str) -> Result<Vec<u8>, CallError> {
        let s = self.server.lock();
        if s.offline {
            return Err(CallError::Offline("the fake server is offline".into()));
        }
        let id = url.strip_prefix("fake://storage/").unwrap_or_default();
        s.uploads.get(id).cloned().ok_or_else(|| refused(format!("no file at {url}")))
    }

    fn waker(&self) -> Waker {
        let wake = self.wake.clone();
        Arc::new(move || {
            *wake.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
            wake.1.notify_one();
        })
    }

    fn park(&mut self, timeout: Duration) {
        let (flag, cv) = &*self.wake;
        let mut woken = flag.lock().unwrap_or_else(|e| e.into_inner());
        if !*woken {
            woken = cv.wait_timeout(woken, timeout).unwrap_or_else(|e| e.into_inner()).0;
        }
        *woken = false;
    }
}

impl Drop for FakeBackend {
    fn drop(&mut self) {
        self.server.lock().subscribers.retain(|(id, _, _)| *id != self.id);
    }
}

#[cfg(feature = "convex")]
pub use convex_backend::ConvexBackend;

#[cfg(feature = "convex")]
mod convex_backend {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    use convex::{
        AuthTokenFetcher, AuthenticationToken, ConvexClient, ConvexClientBuilder, FunctionResult,
        WebSocketState,
    };
    use futures::StreamExt;
    use tokio::runtime::Runtime;
    use tokio::sync::Notify;

    /// How long a call waits for an answer before the engine treats the server as unreachable.
    /// The crate keeps the request and sends it on reconnect; the writes are idempotent.
    const CALL_TIMEOUT: Duration = Duration::from_secs(20);
    /// A call fails as offline once the socket has been down this long: the crate would hold it
    /// until a reconnect, and the engine would publish nothing meanwhile.
    const DOWN_GRACE: Duration = Duration::from_secs(3);

    /// The `convex` crate on a current-thread tokio runtime, driven on the engine's thread.
    pub struct ConvexBackend {
        url: String,
        rt: Runtime,
        client: Option<ConvexClient>,
        notify: Arc<Notify>,
        feed: Option<tokio::task::JoinHandle<()>>,
        /// The socket's state, from the crate's state channel (it reconnects on its own).
        connected: Arc<AtomicBool>,
        /// Since when the socket hasn't been connected (from the client's creation until the first
        /// connect), or `None` while it is.
        down_since: Arc<Mutex<Option<Instant>>>,
    }

    impl ConvexBackend {
        /// Connects lazily, on the sync thread, the first time it is used.
        pub fn new(url: &str) -> Result<Self> {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            let connected = Arc::new(AtomicBool::new(true));
            let down_since = Arc::new(Mutex::new(None));
            Ok(Self {
                url: url.into(),
                rt,
                client: None,
                notify: Arc::default(),
                feed: None,
                connected,
                down_since,
            })
        }

        fn client(&mut self) -> Result<ConvexClient, CallError> {
            if self.client.is_none() {
                let (tx, mut states) = tokio::sync::mpsc::channel(8);
                let builder = ConvexClientBuilder::new(&self.url).with_on_state_change(tx);
                // Opening never fails for want of a network: the crate keeps reconnecting.
                let client =
                    self.rt.block_on(builder.build()).map_err(|e| CallError::Refused(format!("{e:#}")))?;
                let (connected, notify) = (self.connected.clone(), self.notify.clone());
                let down_since = self.down_since.clone();
                *down_since.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                self.rt.spawn(async move {
                    while let Some(state) = states.recv().await {
                        let up = matches!(state, WebSocketState::Connected);
                        connected.store(up, Ordering::Relaxed);
                        let mut down = down_since.lock().unwrap_or_else(|e| e.into_inner());
                        *down = if up { None } else { down.or_else(|| Some(Instant::now())) };
                        drop(down);
                        notify.notify_one();
                    }
                });
                self.client = Some(client);
            }
            Ok(self.client.clone().expect("just connected"))
        }

        fn run(&mut self, kind: CallKind, name: &str, args: Value) -> Result<Value, CallError> {
            let mut client = self.client()?;
            let args = to_convex(args);
            let down_since = self.down_since.clone();
            let down_too_long = async move {
                loop {
                    let since = *down_since.lock().unwrap_or_else(|e| e.into_inner());
                    if since.is_some_and(|t| t.elapsed() >= DOWN_GRACE) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            };
            // The timer is made inside the runtime, which it needs.
            let answer = self.rt.block_on(async {
                let call = std::pin::pin!(tokio::time::timeout(CALL_TIMEOUT, async {
                    match kind {
                        CallKind::Query => client.query(name, args).await,
                        CallKind::Mutation => client.mutation(name, args).await,
                        CallKind::Action => client.action(name, args).await,
                    }
                }));
                match futures::future::select(call, std::pin::pin!(down_too_long)).await {
                    futures::future::Either::Left((answer, _)) => Some(answer),
                    futures::future::Either::Right(_) => None,
                }
            });
            let Some(answer) = answer else {
                return Err(CallError::Offline("Can't reach Linger's server".into()));
            };
            match answer {
                Err(_) => {
                    Err(CallError::Offline(format!("no answer from Convex in {} s", CALL_TIMEOUT.as_secs())))
                }
                Ok(Err(e)) => Err(CallError::Offline(format!("{e:#}"))),
                Ok(Ok(r)) => result(r),
            }
        }
    }

    fn result(r: FunctionResult) -> Result<Value, CallError> {
        match r {
            FunctionResult::Value(v) => Ok(to_json(v)),
            FunctionResult::ErrorMessage(m) => Err(CallError::Refused(m)),
            FunctionResult::ConvexError(e) => Err(CallError::Refused(e.message)),
        }
    }

    /// Convex numbers are floats; whole ones become JSON integers, as the records' `u64`s expect.
    fn to_json(v: convex::Value) -> Value {
        match v {
            convex::Value::Float64(f) if f.fract() == 0.0 && f.abs() < 9e15 => json!(f as i64),
            convex::Value::Float64(f) => json!(f),
            convex::Value::Int64(i) => json!(i),
            convex::Value::Null => Value::Null,
            convex::Value::Boolean(b) => json!(b),
            convex::Value::String(s) => json!(s),
            convex::Value::Bytes(b) => json!(b),
            convex::Value::Array(a) => Value::Array(a.into_iter().map(to_json).collect()),
            convex::Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, to_json(v))).collect()),
        }
    }

    /// JSON arguments as Convex's (numbers as floats, which `v.number()` expects).
    fn to_convex(args: Value) -> BTreeMap<String, convex::Value> {
        match convex::Value::try_from(args) {
            Ok(convex::Value::Object(o)) => o,
            _ => BTreeMap::new(),
        }
    }

    impl Backend for ConvexBackend {
        fn set_auth(&mut self, tokens: Arc<dyn TokenSource>) {
            let Ok(mut client) = self.client() else { return };
            let fetcher: AuthTokenFetcher = Box::new(move |force_refresh| {
                let tokens = tokens.clone();
                Box::pin(async move {
                    let token = if force_refresh { tokens.refresh() } else { tokens.token() }?;
                    Ok(AuthenticationToken::User(token))
                })
            });
            self.rt.block_on(client.set_auth_callback(Some(fetcher)));
        }

        fn subscribe_changes(&mut self, after: u64, sink: Sink) {
            if let Some(old) = self.feed.take() {
                old.abort();
            }
            let mut client = match self.client() {
                Ok(c) => c,
                Err(e) => return sink(Err(e)),
            };
            let args = to_convex(json!({ "after": after }));
            self.feed = Some(self.rt.spawn(async move {
                let mut sub = match client.subscribe(FEED, args).await {
                    Ok(sub) => sub,
                    Err(e) => return sink(Err(CallError::Offline(format!("{e:#}")))),
                };
                while let Some(r) = sub.next().await {
                    sink(result(r));
                }
            }));
        }

        fn mutate(&mut self, name: &str, args: Value) -> Result<Value, CallError> {
            self.run(CallKind::Mutation, name, args)
        }

        fn query(&mut self, name: &str, args: Value) -> Result<Value, CallError> {
            self.run(CallKind::Query, name, args)
        }

        fn action(&mut self, name: &str, args: Value) -> Result<Value, CallError> {
            self.run(CallKind::Action, name, args)
        }

        fn upload_url(&mut self) -> Result<String, CallError> {
            let url = self.mutate("library/files:uploadUrl", json!({}))?;
            url.as_str()
                .map(str::to_string)
                .ok_or_else(|| CallError::Refused(format!("not an upload URL: {url}")))
        }

        fn upload(&mut self, url: &str, bytes: Vec<u8>, content_type: &str) -> Result<String, CallError> {
            let mut resp = ureq::post(url)
                .header("content-type", content_type)
                .send(&bytes[..])
                .map_err(|e| CallError::Offline(format!("{e:#}")))?;
            let body = resp.body_mut().read_to_string().map_err(|e| CallError::Offline(format!("{e:#}")))?;
            let v: Value = serde_json::from_str(&body).map_err(|e| CallError::Refused(format!("{e:#}")))?;
            v["storageId"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| CallError::Refused(format!("no storageId: {body}")))
        }

        fn download(&mut self, url: &str) -> Result<Vec<u8>, CallError> {
            let mut resp = ureq::get(url).call().map_err(|e| match e {
                ureq::Error::StatusCode(code) => CallError::Refused(format!("HTTP {code} for a file")),
                e => CallError::Offline(format!("{e:#}")),
            })?;
            resp.body_mut().read_to_vec().map_err(|e| CallError::Offline(format!("{e:#}")))
        }

        fn connected(&self) -> bool {
            self.connected.load(Ordering::Relaxed)
        }

        fn waker(&self) -> Waker {
            let notify = self.notify.clone();
            Arc::new(move || notify.notify_one())
        }

        fn park(&mut self, timeout: Duration) {
            let notify = self.notify.clone();
            self.rt.block_on(async move {
                let woken = std::pin::pin!(notify.notified());
                let slept = std::pin::pin!(tokio::time::sleep(timeout));
                futures::future::select(woken, slept).await;
            });
        }
    }
}
