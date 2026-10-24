//! Ask on the iPhone (M3b's FFI 9 and 10): the library search the jump field and the Ask tools
//! share, and [`Conversation`], the desktop's assistant loop with its device-side tools, streaming
//! what it does through an [`AskObserver`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use linger_core::api::{self, Api, ApiError};
use linger_core::assistant::{self, Event, Mode, Source, Stopped};
use linger_core::chat::{self, Message, Role, Segment};
use linger_core::records;
use serde_json::json;

use crate::items::{ItemKind, ItemRange, title_of};
use crate::study::{ChatMessage, LibrarySync, Study, TokenProvider, Tokens, core_verse, failed};
use crate::{BibleError, BibleLibrary, Reference, chapter_idx};

/// A passage of the library that matches a search.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LibraryHit {
    pub uid: String,
    pub kind: ItemKind,
    pub title: String,
    /// Who it is from: a sermon's speaker and church.
    pub source: Option<String>,
    /// When the item was recorded or made, in server milliseconds.
    pub date: u64,
    /// The matching passage.
    pub text: String,
    /// Where in a recording the passage is.
    pub start_ms: Option<u64>,
    /// Which page of a photographed item it is on.
    pub page: Option<u32>,
    pub unreviewed: bool,
}

/// What a library search found; `offline` when only the items cached here were searched.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LibrarySearch {
    pub hits: Vec<LibraryHit>,
    pub offline: bool,
}

impl From<records::SearchHit> for LibraryHit {
    fn from(h: records::SearchHit) -> Self {
        Self {
            uid: h.item_uid,
            kind: h.kind.into(),
            title: h.title,
            source: h.source,
            date: h.date,
            text: h.text,
            start_ms: h.start_ms,
            page: h.page,
            unreviewed: h.unreviewed,
        }
    }
}

#[uniffi::export]
impl LibrarySync {
    /// Searches the library as Ask's `search_library` does: on the server through this engine's
    /// connection, or the cached items' words when offline. Every word must appear, the last as a
    /// prefix. Blocks: call it off the main thread.
    pub fn search_library(&self, query: String, limit: u32) -> Result<LibrarySearch, BibleError> {
        let library = self.handle().user_library().map_err(failed)?;
        let (hits, offline) =
            library.search(json!({ "query": query, "limit": limit.clamp(1, 50) })).map_err(failed)?;
        let study = self.study().lock();
        let hits = hits
            .into_iter()
            .map(|h| {
                let mut hit = LibraryHit::from(h);
                // Offline, an untitled note comes back untitled: name it as lists do.
                if hit.title.trim().is_empty()
                    && let Ok(Some(item)) = study.item(&hit.uid)
                {
                    hit.title = title_of(&item);
                }
                hit
            })
            .collect();
        Ok(LibrarySearch { hits, offline })
    }
}

/// Where an answer may come from: the Bible and the user's library (the default), or the Bible
/// only, which keeps the library tools from running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AskScope {
    BibleAndLibrary,
    BibleOnly,
}

/// Standard questions, or Deep study (Pro; counts as two).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AskMode {
    Standard,
    Deep,
}

/// What the user is looking at when they ask.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AskContext {
    pub book: u8,
    pub chapter: u8,
    /// The reader's translation: only its name goes to the model, and the tools read it only if
    /// it is one Ask may quote (core's `ask_translation`).
    pub translation: String,
    pub selection: Option<ItemRange>,
    /// The library item asked about, from its page.
    pub item: Option<String>,
}

/// Something an answer drew on: a passage of the Bible, or an item of the library.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum AskSource {
    Passage { reference: Reference },
    Item { uid: String, kind: ItemKind, title: String, at_ms: Option<u64>, page: Option<u32> },
}

/// What Ask hears while an answer is coming. Called on the Ask thread.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum AskEvent {
    /// More of the answer's text.
    Text { text: String },
    /// What it is doing: "Reading John 3:16", "Searching your library for “grace”", "Thinking".
    Status { label: String },
    /// A tool read a passage or a library item for the answer.
    Read { source: AskSource },
    /// Questions left this month, and whether this one went to a lighter model.
    Allowance { questions_left: Option<u32>, downgraded: bool },
}

#[uniffi::export(with_foreign)]
pub trait AskObserver: Send + Sync {
    fn event(&self, event: AskEvent);
}

/// Why a question got no answer.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AskError {
    /// Not signed in, or the session ended.
    #[error("{message}")]
    SignIn { message: String },
    /// The month's questions are spent (402) or the feature is Pro's (403); `upgrade` when Pro
    /// would help.
    #[error("{message}")]
    Upgrade { message: String, upgrade: bool, plan_required: bool },
    /// Rate limited: try again after `retry_after` seconds.
    #[error("{message}")]
    Wait { message: String, retry_after: Option<u64> },
    /// This app is too old for the proxy's Ask contract.
    #[error("{message}")]
    Update { message: String },
    /// The conversation is too long to carry on.
    #[error("{message}")]
    TooLarge { message: String },
    /// [`Conversation::stop`] ended it.
    #[error("stopped")]
    Stopped,
    #[error("{message}")]
    Failed { message: String },
}

impl From<anyhow::Error> for AskError {
    fn from(e: anyhow::Error) -> Self {
        if e.is::<Stopped>() {
            return Self::Stopped;
        }
        match e.downcast::<ApiError>() {
            Ok(e) => {
                let message = e.to_string();
                match e {
                    ApiError::SignIn(_) => Self::SignIn { message },
                    ApiError::Upgrade { message, upgrade, plan_required } => {
                        Self::Upgrade { message, upgrade, plan_required }
                    }
                    ApiError::Wait { message, retry_after } => Self::Wait { message, retry_after },
                    ApiError::Update(_) => Self::Update { message },
                    ApiError::TooLarge(_) => Self::TooLarge { message },
                    _ => Self::Failed { message },
                }
            }
            Err(e) => Self::Failed { message: format!("{e:#}") },
        }
    }
}

/// An answer, with what it drew on: the passages and items it read, then those it cites, each
/// once.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AskAnswer {
    /// The answer's text, with `{{…}}` citations (`citation_segments`).
    pub text: String,
    pub sources: Vec<AskSource>,
}

/// One Ask conversation, kept in Recents once answered. Its calls block: make them off the main
/// thread.
#[derive(uniffi::Object)]
pub struct Conversation {
    api: Api,
    /// The API's view: every block as it came back.
    inner: Mutex<assistant::Conversation>,
    /// What the chat shows.
    messages: Mutex<Vec<Message>>,
    /// The saved chat it continues, and its title.
    saved: Mutex<Option<(i64, String)>>,
    stop: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[uniffi::export]
impl Conversation {
    /// A new conversation through the proxy at `base_url` (production when `None`), as the user
    /// `tokens` signs in.
    #[uniffi::constructor]
    pub fn new(base_url: Option<String>, tokens: Arc<dyn TokenProvider>) -> Arc<Self> {
        Arc::new(Self::with(base_url, tokens, assistant::Conversation::new(), Vec::new(), None))
    }

    /// A saved conversation from Recents, to carry on. Fails for one that is listed but isn't on
    /// this device yet.
    #[uniffi::constructor]
    pub fn open(
        study: Arc<Study>,
        id: i64,
        base_url: Option<String>,
        tokens: Arc<dyn TokenProvider>,
    ) -> Result<Arc<Self>, BibleError> {
        let saved = study.lock().chat(id).ok_or_else(|| BibleError::Failed {
            message: "This conversation isn't on this iPhone yet.".into(),
        })?;
        Ok(Arc::new(Self::with(
            base_url,
            tokens,
            assistant::Conversation::from_json(&saved.api),
            saved.messages,
            Some((saved.id, saved.title)),
        )))
    }

    /// The questions and answers so far.
    pub fn messages(&self) -> Vec<ChatMessage> {
        lock(&self.messages)
            .iter()
            .map(|m| ChatMessage { from_user: m.role == Role::User, text: m.text.clone() })
            .collect()
    }

    /// The chat in Recents this is saved as, once answered.
    pub fn chat_id(&self) -> Option<i64> {
        lock(&self.saved).as_ref().map(|(id, _)| *id)
    }

    /// Asks `question` about `context`, reporting to `observer` until the answer is done; then
    /// saves the conversation in Recents. The tools read the Bible here and, unless the scope is
    /// the Bible only, the library through `sync`.
    #[allow(clippy::too_many_arguments)]
    pub fn ask(
        &self,
        bible: Arc<BibleLibrary>,
        study: Arc<Study>,
        sync: Option<Arc<LibrarySync>>,
        question: String,
        context: Option<AskContext>,
        scope: AskScope,
        mode: AskMode,
        observer: Arc<dyn AskObserver>,
    ) -> Result<AskAnswer, AskError> {
        self.stop.store(false, Ordering::Relaxed);
        let reading = context.as_ref().map_or("", |c| c.translation.as_str());
        let translation = bible
            .inner
            .load(chat::ask_translation(reading))
            .map_err(|e| AskError::Failed { message: format!("{e:#}") })?;
        let library = match (scope, &sync) {
            (AskScope::BibleAndLibrary, Some(sync)) => {
                Some(sync.handle().user_library().map_err(AskError::from)?)
            }
            _ => None,
        };
        let tools = chat::Tools { translation, library, church: None };
        let context = context.map(|c| self.context(&bible, &study, c)).transpose()?;
        let cfg = assistant::Config::proxy(
            self.api.clone(),
            match mode {
                AskMode::Standard => Mode::Standard,
                AskMode::Deep => Mode::Deep,
            },
        );
        let mut read = Vec::new();
        let answer =
            lock(&self.inner).ask_until(&cfg, &tools, context.as_ref(), &question, &self.stop, |e| {
                let event = match e {
                    Event::Text(text) => AskEvent::Text { text },
                    Event::Tool(label) => AskEvent::Status { label },
                    Event::Thinking => AskEvent::Status { label: "Thinking".into() },
                    Event::Allowance { questions_left, downgraded } => {
                        AskEvent::Allowance { questions_left, downgraded }
                    }
                    Event::Read(source) => {
                        let source = from_core(source);
                        read.push(source.clone());
                        AskEvent::Read { source }
                    }
                };
                observer.event(event);
            })?;
        let sources = sources(&study, read, &answer);
        let text = answer.text.clone();
        lock(&self.messages).extend([Message::user(question), answer]);
        self.save(&study)?;
        Ok(AskAnswer { text, sources })
    }

    /// Stops the answer being written: `ask` returns `Stopped`, and the question stays out of
    /// the conversation.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Conversation {
    fn with(
        base_url: Option<String>,
        tokens: Arc<dyn TokenProvider>,
        inner: assistant::Conversation,
        messages: Vec<Message>,
        saved: Option<(i64, String)>,
    ) -> Self {
        let base = base_url.map_or_else(api::base_url, |u| u.trim_end_matches('/').to_string());
        Self {
            api: Api::new(base, Arc::new(Tokens(tokens))),
            inner: Mutex::new(inner),
            messages: Mutex::new(messages),
            saved: Mutex::new(saved),
            stop: AtomicBool::new(false),
        }
    }

    fn context(&self, bible: &BibleLibrary, study: &Study, c: AskContext) -> Result<chat::Context, AskError> {
        let chapter =
            chapter_idx(c.book, c.chapter).map_err(|e| AskError::Failed { message: e.to_string() })?;
        let selection = c
            .selection
            .map(|r| Ok::<_, BibleError>((core_verse(r.from)?, core_verse(r.to)?)))
            .transpose()
            .map_err(|e| AskError::Failed { message: e.to_string() })?;
        let reading = bible
            .inner
            .get(&c.translation)
            .map_or_else(|| c.translation.to_uppercase(), |e| e.meta.abbrev.clone());
        let item = match c.item {
            Some(uid) => {
                let item =
                    study.lock().item(&uid).map_err(|e| AskError::Failed { message: format!("{e:#}") })?;
                item.map(|i| {
                    let title = title_of(&i);
                    (i.uid, title)
                })
            }
            None => None,
        };
        Ok(chat::Context { chapter, selection, reading, item })
    }

    /// Saves the conversation in Recents: new, titled with its first question, or over the chat
    /// it continues.
    fn save(&self, study: &Study) -> Result<(), AskError> {
        let messages = lock(&self.messages).clone();
        let api = lock(&self.inner).to_json();
        let mut saved = lock(&self.saved);
        let title = saved.as_ref().map_or_else(
            || {
                chat::title_for(
                    messages.iter().find(|m| m.role == Role::User).map_or("", |m| m.text.as_str()),
                )
            },
            |(_, t)| t.clone(),
        );
        let mut id = 0;
        study
            .write(|s| {
                id = s
                    .save_chat(saved.as_ref().map(|(id, _)| *id), &title, &messages, &api)
                    .map_err(failed)?;
                Ok(())
            })
            .map_err(|e| AskError::Failed { message: e.to_string() })?;
        *saved = Some((id, title));
        Ok(())
    }
}

fn from_core(source: Source) -> AskSource {
    match source {
        Source::Passage(r) => AskSource::Passage { reference: r.into() },
        Source::Item { uid, kind, title, at_ms } => {
            AskSource::Item { uid, kind: kind.into(), title, at_ms, page: None }
        }
    }
}

/// What was read, then what the answer cites that wasn't, each once. A cited item is named from
/// the cache; one that isn't there is left out, since nothing could open it.
fn sources(study: &Study, read: Vec<AskSource>, answer: &Message) -> Vec<AskSource> {
    let mut out = Vec::new();
    let push = |s: AskSource, out: &mut Vec<AskSource>| {
        let same = |o: &AskSource| match (o, &s) {
            (AskSource::Item { uid: a, .. }, AskSource::Item { uid: b, .. }) => a == b,
            (a, b) => a == b,
        };
        if !out.iter().any(same) {
            out.push(s);
        }
    };
    for s in read {
        push(s, &mut out);
    }
    for seg in chat::segments(&answer.text) {
        match seg {
            Segment::Verse(r) => push(AskSource::Passage { reference: r.into() }, &mut out),
            Segment::Item { uid, at_ms, page } => {
                if let Ok(Some(item)) = study.lock().item(&uid) {
                    push(
                        AskSource::Item {
                            uid: item.uid,
                            kind: item.kind.into(),
                            title: item.title,
                            at_ms,
                            page,
                        },
                        &mut out,
                    );
                }
            }
            Segment::Text(_) | Segment::Sermon { .. } => {}
        }
    }
    out
}

/// The line under an answer that folds its sources. While it is written, the library items read
/// so far ("Reading 1 sermon and 1 note from your library…"), or `None` when none have been: the
/// latest status shows instead. Once done, "Read 1 sermon and 1 note from your library", or
/// without the library "From the Bible: Romans 8:28 and Genesis 50:20" ("…: 3 passages" past
/// two); `None` with no sources.
#[uniffi::export]
pub fn ask_sources_line(sources: Vec<AskSource>, done: bool) -> Option<String> {
    let mut kinds: Vec<(&str, usize)> = Vec::new();
    let mut passages = Vec::new();
    for s in &sources {
        match s {
            AskSource::Item { kind, .. } => {
                let name = kind_name(*kind);
                match kinds.iter_mut().find(|(k, _)| *k == name) {
                    Some((_, n)) => *n += 1,
                    None => kinds.push((name, 1)),
                }
            }
            AskSource::Passage { reference } => passages.push(reference),
        }
    }
    if !kinds.is_empty() {
        let parts: Vec<String> =
            kinds.iter().map(|(k, n)| format!("{n} {k}{}", if *n == 1 { "" } else { "s" })).collect();
        let list = match parts.split_last() {
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
            None => unreachable!(),
        };
        return Some(if done {
            format!("Read {list} from your library")
        } else {
            format!("Reading {list} from your library…")
        });
    }
    if !done || passages.is_empty() {
        return None;
    }
    let names: Vec<String> =
        passages.iter().map(|r| crate::format_reference(**r).unwrap_or_default()).collect();
    Some(match names.as_slice() {
        [one] => format!("From the Bible: {one}"),
        [a, b] => format!("From the Bible: {a} and {b}"),
        many => format!("From the Bible: {} passages", many.len()),
    })
}

/// Three questions to start from, drawn from the context and the library: the verses or chapter
/// being read, and an item of the library about them, when the scope reaches it.
#[uniffi::export]
pub fn ask_suggestions(study: Arc<Study>, context: AskContext, scope: AskScope) -> Vec<String> {
    let s = study.lock();
    if let Some(item) = context.item.as_deref().and_then(|uid| s.item(uid).ok().flatten()) {
        let what = kind_name(item.kind.into());
        return vec![
            format!("Summarize this {what}."),
            format!("Which passages does this {what} draw on?"),
            format!("What should I take from this {what}?"),
        ];
    }
    let chapter = crate::format_reference(Reference {
        book: context.book,
        chapter: context.chapter,
        verse: None,
        verse_end: None,
    })
    .unwrap_or_default();
    let (place, mut out) = match context.selection {
        Some(r) => {
            let place = crate::format_reference(Reference {
                book: r.from.book,
                chapter: r.from.chapter,
                verse: Some(r.from.verse),
                verse_end: (r.to != r.from).then_some(r.to.verse),
            })
            .unwrap_or_default();
            let out = vec![
                format!("What does {place} mean?"),
                format!("How does {place} fit in {chapter}?"),
                "Where else does the Bible say this?".to_string(),
            ];
            (place, out)
        }
        None => (
            chapter.clone(),
            vec![
                format!("What is {chapter} about?"),
                format!("What are the key words in {chapter}?"),
                format!("How does {chapter} connect to the rest of the Bible?"),
            ],
        ),
    };
    if scope == AskScope::BibleAndLibrary {
        let (first, last) = match context.selection {
            Some(r) => (r.from.verse, r.to.verse),
            None => (1, u8::MAX),
        };
        let about = chapter_idx(context.book, context.chapter)
            .ok()
            .and_then(|c| s.spans_in_chapter(c).ok())
            .and_then(|spans| spans.into_iter().find(|sp| sp.first <= last && sp.last >= first))
            .and_then(|sp| s.item(&sp.uid).ok().flatten());
        if let Some(item) = about {
            out[2] =
                format!("What does my {} “{}” say about {place}?", kind_name(item.kind.into()), item.title);
        }
    }
    out
}

fn kind_name(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Sermon => "sermon",
        ItemKind::Note => "note",
        ItemKind::Handwritten | ItemKind::Page => "photo",
        ItemKind::Slide => "slide",
    }
}

/// The Ask consent sheet's words (core's `privacy`, as on the desktop).
#[uniffi::export]
pub fn ask_consent_text() -> crate::ConsentText {
    use linger_core::privacy::{ASK_CONSENT_POINTS, ASK_CONSENT_TITLE};
    crate::ConsentText {
        title: ASK_CONSENT_TITLE.into(),
        points: ASK_CONSENT_POINTS.map(String::from).to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    use linger_core::sync::FakeServer;
    use serde_json::Value;

    use super::*;
    use crate::study::VerseRef;

    struct Signed;

    impl TokenProvider for Signed {
        fn token(&self) -> Option<String> {
            Some("t".into())
        }
        fn refresh(&self) -> Option<String> {
            None
        }
    }

    #[derive(Default)]
    struct Heard(Mutex<Vec<AskEvent>>, Mutex<Option<Arc<Conversation>>>);

    impl AskObserver for Heard {
        fn event(&self, event: AskEvent) {
            // Stops at the first word, when told which conversation to stop.
            if matches!(event, AskEvent::Text { .. })
                && let Some(c) = lock(&self.1).as_ref()
            {
                c.stop();
            }
            lock(&self.0).push(event);
        }
    }

    /// One reply of the fake proxy: a status, extra headers, and the body, sent in pieces with a
    /// pause between them.
    struct Reply {
        status: u16,
        headers: &'static str,
        pieces: Vec<String>,
        pause: Duration,
    }

    fn sse(events: &[Value]) -> Reply {
        Reply {
            status: 200,
            headers: "content-type: text/event-stream\r\nx-linger-questions-left: 22\r\n",
            pieces: events
                .iter()
                .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
                .collect(),
            pause: Duration::ZERO,
        }
    }

    /// Answers `/v1/ask` with `replies` in turn, and hands back each request's body.
    fn proxy(replies: Vec<Reply>) -> (String, Receiver<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for reply in replies {
                let Ok((mut conn, _)) = listener.accept() else { return };
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let _ = tx.send(serde_json::from_slice::<Value>(&body).unwrap());
                let head =
                    format!("HTTP/1.1 {} X\r\n{}connection: close\r\n\r\n", reply.status, reply.headers);
                if conn.write_all(head.as_bytes()).is_err() {
                    continue;
                }
                for piece in reply.pieces {
                    if conn.write_all(piece.as_bytes()).and_then(|_| conn.flush()).is_err() {
                        break;
                    }
                    std::thread::sleep(reply.pause);
                }
            }
        });
        (url, rx)
    }

    fn tool_calls(calls: &[(&str, Value)]) -> Vec<Value> {
        let mut events = vec![json!({"type":"message_start","message":{}})];
        for (i, (name, input)) in calls.iter().enumerate() {
            events.push(json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":format!("toolu_{i}"),"name":name,"input":{}}}));
            events.push(json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":input.to_string()}}));
            events.push(json!({"type":"content_block_stop","index":i}));
        }
        events.push(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}));
        events.push(json!({"type":"message_stop"}));
        events
    }

    fn text(words: &[&str]) -> Vec<Value> {
        let mut events = vec![
            json!({"type":"message_start","message":{}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ];
        for w in words {
            events
                .push(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":w}}));
        }
        events.push(json!({"type":"content_block_stop","index":0}));
        events.push(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}));
        events.push(json!({"type":"message_stop"}));
        events
    }

    struct Device {
        dir: PathBuf,
        bible: Arc<BibleLibrary>,
        study: Arc<Study>,
        sync: Arc<LibrarySync>,
        _server: Arc<FakeServer>,
    }

    impl Device {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("linger-ffi-ask-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let path = dir.to_string_lossy().into_owned();
            let server = FakeServer::new();
            let study = Study::new(path.clone(), None).unwrap();
            let sync =
                LibrarySync::with_backend(study.clone(), Box::new(server.backend()), Arc::new(Signed), None)
                    .unwrap();
            Self { dir, bible: Arc::new(BibleLibrary::new(path, None)), study, sync, _server: server }
        }

        /// A note on Romans 8:28, on the server as well as here.
        fn note(&self) -> String {
            let rom = VerseRef { book: 44, chapter: 8, verse: 28 };
            let uid = self
                .study
                .save_note_item(
                    "Joseph: what they meant for evil, God meant for good.".into(),
                    vec![ItemRange { from: rom, to: rom }],
                )
                .unwrap();
            assert_eq!(self.sync.wait_idle(5_000), crate::SyncState::Synced);
            uid
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn romans_8_28() -> AskContext {
        let v = VerseRef { book: 44, chapter: 8, verse: 28 };
        AskContext {
            book: 44,
            chapter: 8,
            translation: "kjv".into(),
            selection: Some(ItemRange { from: v, to: v }),
            item: None,
        }
    }

    fn rom(verse: u8) -> Reference {
        Reference { book: 44, chapter: 8, verse: Some(verse), verse_end: None }
    }

    #[test]
    fn an_answer_streams_reads_a_passage_and_a_note_cites_them_and_is_kept_in_recents() {
        let d = Device::new("answer");
        let uid = d.note();
        let (url, bodies) = proxy(vec![
            sse(&tool_calls(&[
                ("get_passage", json!({"reference":"Romans 8:28"})),
                ("read_item", json!({"item": uid})),
            ])),
            sse(&text(&[
                "{{Romans 8:28}} promises good. ",
                &format!("So does your note {{{{item {}}}}}.", &uid[..8]),
            ])),
        ]);
        let conversation = Conversation::new(Some(url), Arc::new(Signed));
        let heard = Arc::new(Heard::default());
        let answer = conversation
            .ask(
                d.bible.clone(),
                d.study.clone(),
                Some(d.sync.clone()),
                "What does 8:28 really promise?".into(),
                Some(romans_8_28()),
                AskScope::BibleAndLibrary,
                AskMode::Standard,
                heard.clone(),
            )
            .unwrap();

        let note = AskSource::Item {
            uid: uid.clone(),
            kind: ItemKind::Note,
            title: "Joseph: what they meant for evil, God meant for good.".into(),
            at_ms: None,
            page: None,
        };
        let heard = lock(&heard.0).clone();
        assert_eq!(heard[0], AskEvent::Allowance { questions_left: Some(22), downgraded: false });
        let reads: Vec<_> = heard
            .iter()
            .filter_map(|e| match e {
                AskEvent::Read { source } => Some(source.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(reads, [AskSource::Passage { reference: rom(28) }, note.clone()]);
        assert!(heard.contains(&AskEvent::Status { label: "Reading Romans 8:28".into() }), "{heard:?}");
        assert!(heard.contains(&AskEvent::Text { text: "{{Romans 8:28}} promises good. ".into() }));
        // Cited again in the text, each source still shows once.
        assert_eq!(answer.sources, [AskSource::Passage { reference: rom(28) }, note]);
        assert_eq!(ask_sources_line(reads, false).as_deref(), Some("Reading 1 note from your library…"));
        assert_eq!(
            ask_sources_line(answer.sources.clone(), true).as_deref(),
            Some("Read 1 note from your library")
        );

        // The first round says what is read and selected; the second carries the tools' results,
        // the passage from the KJV the reader shows.
        let first = bodies.recv().unwrap();
        assert_eq!(
            (first["contract"].clone(), first["round"].clone()),
            (json!(linger_core::contract::VERSION), json!(0))
        );
        let about = first["messages"][0]["content"][0]["text"].as_str().unwrap();
        assert!(
            about.starts_with("The user is reading Romans 8 in the KJV. They have selected Romans 8:28."),
            "{about}"
        );
        assert!(about.contains("The user's library has 1 item; 1 of them touches Romans 8."), "{about}");
        let second = bodies.recv().unwrap();
        assert_eq!(second["round"], 1);
        let results = second["messages"][2]["content"].as_array().unwrap();
        assert!(
            results[0]["content"]
                .as_str()
                .unwrap()
                .contains("And we know that all things work together for good")
        );
        assert!(results[1]["content"].as_str().unwrap().contains("God meant for good"));

        // Kept in Recents, titled with the question, and carried on from there.
        let chats = d.study.chats();
        assert_eq!((chats.len(), chats[0].title.as_str()), (1, "What does 8:28 really promise?"));
        assert_eq!(conversation.chat_id(), Some(chats[0].id));
        let reopened = Conversation::open(d.study.clone(), chats[0].id, None, Arc::new(Signed)).unwrap();
        assert_eq!(reopened.messages(), conversation.messages());
        assert_eq!(
            reopened.messages()[0],
            ChatMessage { from_user: true, text: "What does 8:28 really promise?".into() }
        );
        assert_eq!(reopened.messages()[1].text, answer.text);
    }

    #[test]
    fn bible_only_keeps_the_library_tools_from_running() {
        let d = Device::new("bible-only");
        d.note();
        let (url, bodies) = proxy(vec![
            sse(&tool_calls(&[("search_library", json!({"query":"Joseph"}))])),
            sse(&text(&["See {{Genesis 50:20}}."])),
        ]);
        let answer = Conversation::new(Some(url), Arc::new(Signed))
            .ask(
                d.bible.clone(),
                d.study.clone(),
                Some(d.sync.clone()),
                "Who meant it for good?".into(),
                Some(romans_8_28()),
                AskScope::BibleOnly,
                AskMode::Standard,
                Arc::new(Heard::default()),
            )
            .unwrap();
        let about = bodies.recv().unwrap()["messages"][0]["content"][0]["text"].as_str().unwrap().to_string();
        assert!(!about.contains("library"), "the library isn't mentioned: {about}");
        let result = bodies.recv().unwrap()["messages"][2]["content"][0].clone();
        assert_eq!(result["is_error"], true);
        assert!(result["content"].as_str().unwrap().contains("from the Bible only"), "{result}");
        assert_eq!(
            answer.sources,
            [AskSource::Passage {
                reference: Reference { book: 0, chapter: 50, verse: Some(20), verse_end: None }
            }]
        );
        assert_eq!(ask_sources_line(answer.sources, true).as_deref(), Some("From the Bible: Genesis 50:20"));
    }

    #[test]
    fn stop_ends_the_answer_and_keeps_the_question_out() {
        let d = Device::new("stop");
        let mut slow = sse(&text(&["One ", "two ", "three ", "four."]));
        slow.pause = Duration::from_millis(150);
        let (url, _bodies) = proxy(vec![slow]);
        let conversation = Conversation::new(Some(url), Arc::new(Signed));
        let heard = Arc::new(Heard::default());
        *lock(&heard.1) = Some(conversation.clone());
        let e = conversation
            .ask(
                d.bible.clone(),
                d.study.clone(),
                None,
                "Count".into(),
                None,
                AskScope::BibleAndLibrary,
                AskMode::Standard,
                heard.clone(),
            )
            .unwrap_err();
        assert!(matches!(e, AskError::Stopped), "{e:?}");
        let words = lock(&heard.0).iter().filter(|e| matches!(e, AskEvent::Text { .. })).count();
        assert_eq!(words, 1, "nothing streams after the stop");
        assert!(conversation.messages().is_empty());
        assert!(d.study.chats().is_empty(), "a stopped question isn't kept");
    }

    #[test]
    fn a_spent_allowance_comes_back_as_upgrade() {
        let d = Device::new("spent");
        let (url, _bodies) = proxy(vec![Reply {
            status: 402,
            headers: "content-type: application/json\r\n",
            pieces: vec![r#"{"error":{"type":"allowance_exhausted","message":"You've used this month's free questions.","upgrade":true}}"#.into()],
            pause: Duration::ZERO,
        }]);
        let conversation = Conversation::new(Some(url), Arc::new(Signed));
        let e = conversation
            .ask(
                d.bible.clone(),
                d.study.clone(),
                None,
                "Why?".into(),
                None,
                AskScope::BibleAndLibrary,
                AskMode::Standard,
                Arc::new(Heard::default()),
            )
            .unwrap_err();
        assert!(
            matches!(&e, AskError::Upgrade { message, upgrade: true, plan_required: false } if message == "You've used this month's free questions."),
            "{e:?}"
        );
        assert!(conversation.messages().is_empty());
    }

    #[test]
    fn the_library_search_finds_a_note_and_suggestions_draw_on_it() {
        let d = Device::new("search");
        d.note();
        let found = d.sync.search_library("joseph good".into(), 10).unwrap();
        assert_eq!(found.hits.len(), 1);
        assert_eq!(
            (found.hits[0].kind, found.hits[0].title.as_str()),
            (ItemKind::Note, "Joseph: what they meant for evil, God meant for good.")
        );
        assert!(d.sync.search_library("pharaoh".into(), 10).unwrap().hits.is_empty());

        assert_eq!(
            ask_suggestions(d.study.clone(), romans_8_28(), AskScope::BibleAndLibrary),
            [
                "What does Romans 8:28 mean?",
                "How does Romans 8:28 fit in Romans 8?",
                "What does my note “Joseph: what they meant for evil, God meant for good.” say about Romans 8:28?",
            ]
        );
        // Not with the Bible only, nor on a verse the note isn't on.
        assert_eq!(
            ask_suggestions(d.study.clone(), romans_8_28(), AskScope::BibleOnly)[2],
            "Where else does the Bible say this?"
        );
        let v29 = VerseRef { book: 44, chapter: 8, verse: 29 };
        let other = AskContext { selection: Some(ItemRange { from: v29, to: v29 }), ..romans_8_28() };
        assert_eq!(
            ask_suggestions(d.study.clone(), other, AskScope::BibleAndLibrary)[2],
            "Where else does the Bible say this?"
        );
        let chapter = AskContext { selection: None, ..romans_8_28() };
        assert_eq!(
            ask_suggestions(d.study.clone(), chapter, AskScope::BibleAndLibrary)[0],
            "What is Romans 8 about?"
        );
    }

    #[test]
    fn the_sources_line_counts_items_by_kind() {
        let item = |kind| AskSource::Item {
            uid: format!("{kind:?}"),
            kind,
            title: String::new(),
            at_ms: None,
            page: None,
        };
        let mut two = item(ItemKind::Sermon);
        if let AskSource::Item { uid, .. } = &mut two {
            uid.push('2');
        }
        let all = vec![
            item(ItemKind::Sermon),
            item(ItemKind::Note),
            two,
            item(ItemKind::Page),
            AskSource::Passage { reference: rom(28) },
        ];
        assert_eq!(
            ask_sources_line(all, true).as_deref(),
            Some("Read 2 sermons, 1 note and 1 photo from your library")
        );
        let passages = vec![rom(28), rom(29), rom(30)]
            .into_iter()
            .map(|reference| AskSource::Passage { reference })
            .collect::<Vec<_>>();
        assert_eq!(ask_sources_line(passages.clone(), false), None, "the status shows while it's written");
        assert_eq!(ask_sources_line(passages, true).as_deref(), Some("From the Bible: 3 passages"));
        assert_eq!(ask_sources_line(Vec::new(), true), None);
    }
}
