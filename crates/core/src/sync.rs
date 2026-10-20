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

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::auth::{self, TokenSource};
use crate::records::{Changes, VerseId};
use crate::study::cache;

/// The change feed's query.
pub const FEED: &str = "library/sync:changes";
/// The server's clock, from a mutation (never cached).
pub const CLOCK: &str = "library/sync:now";
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
        token_due: None,
        data_changed: false,
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
    /// status at `timeout`.
    pub fn wait_idle(&self, timeout: Duration) -> Result<u64, SyncStatus> {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if state.status == SyncStatus::Synced {
                return Ok(state.cursor);
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
    token_due: Option<Instant>,
    /// The cache changed since the last `on_change`.
    data_changed: bool,
}

impl Engine {
    fn run(&mut self) {
        self.authenticate();
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
            } else if self.caught_up_once {
                if !self.clock_known && self.feed_error.is_none() {
                    self.measure_clock();
                }
                if self.clock_known {
                    self.drain();
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
            Event::Feed(_, Ok(value)) => {
                self.online();
                if let Err(e) = self.apply(value) {
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
        match serde_json::from_value::<Changes>(value)? {
            Changes::Page { records, seq, .. } => {
                if cache::apply_page(&mut self.conn, &records, seq)? > 0 {
                    self.data_changed = true;
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
            let (name, field) = entry.kind.mutation();
            let done = match self.backend.mutate(name, entry.args.clone()) {
                Ok(answer) => {
                    self.online();
                    let seq = answer["seq"].as_u64().unwrap_or(0);
                    self.written = self.written.max(seq);
                    match answer["result"].as_str() {
                        Some("ok") => cache::sent(&mut self.conn, &entry, seq),
                        Some("conflict") => {
                            self.data_changed = true;
                            cache::conflicted(
                                &mut self.conn,
                                &entry,
                                seq,
                                &answer[field],
                                answer["createdAt"].as_u64(),
                            )
                            .map(|_| ())
                        }
                        _ => self.refused(&entry, &format!("unexpected answer {answer}")),
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
        } else if pending > 0 || !self.caught_up || cache::cursor(&self.conn).unwrap_or(0) < self.written {
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
        self.last_write_ms = now_ms();
        self.deliver_all();
        Ok(json!({ "result": "ok", "seq": seq }))
    }
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

    /// The `convex` crate on a current-thread tokio runtime, driven on the engine's thread.
    pub struct ConvexBackend {
        url: String,
        rt: Runtime,
        client: Option<ConvexClient>,
        notify: Arc<Notify>,
        feed: Option<tokio::task::JoinHandle<()>>,
        /// The socket's state, from the crate's state channel (it reconnects on its own).
        connected: Arc<AtomicBool>,
    }

    impl ConvexBackend {
        /// Connects lazily, on the sync thread, the first time it is used.
        pub fn new(url: &str) -> Result<Self> {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            let connected = Arc::new(AtomicBool::new(true));
            Ok(Self { url: url.into(), rt, client: None, notify: Arc::default(), feed: None, connected })
        }

        fn client(&mut self) -> Result<ConvexClient, CallError> {
            if self.client.is_none() {
                let (tx, mut states) = tokio::sync::mpsc::channel(8);
                let builder = ConvexClientBuilder::new(&self.url).with_on_state_change(tx);
                // Opening never fails for want of a network: the crate keeps reconnecting.
                let client =
                    self.rt.block_on(builder.build()).map_err(|e| CallError::Refused(format!("{e:#}")))?;
                let (connected, notify) = (self.connected.clone(), self.notify.clone());
                self.rt.spawn(async move {
                    while let Some(state) = states.recv().await {
                        connected.store(matches!(state, WebSocketState::Connected), Ordering::Relaxed);
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
            // The timer is made inside the runtime, which it needs.
            let answer = self.rt.block_on(async {
                tokio::time::timeout(CALL_TIMEOUT, async {
                    match kind {
                        CallKind::Query => client.query(name, args).await,
                        CallKind::Mutation => client.mutation(name, args).await,
                        CallKind::Action => client.action(name, args).await,
                    }
                })
                .await
            });
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
