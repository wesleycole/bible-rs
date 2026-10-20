//! The study store and the sync engine, as the apps hold them.
//!
//! [`Study`] is the same `StudyStore` the desktop uses (`data_dir/study.sqlite`). [`LibrarySync`]
//! runs the desktop's sync engine over Convex for that store and does the wiring the desktop does
//! each frame: a change from the feed is read into the store, which then tells its observer, and a
//! change made here is sent at once. Both observers are called from background threads.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use linger_core::auth::TokenSource;
use linger_core::books::VerseRef as CoreVerse;
use linger_core::study::{self, StudyStore};
use linger_core::sync::{self, Backend, SyncHandle, SyncStatus};

use crate::{BibleError, chapter_idx};

/// One verse, as the study store keys it: book 0-based, chapter and verse 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct VerseRef {
    pub book: u8,
    pub chapter: u8,
    pub verse: u8,
}

/// Gives sync and the API a bearer token for Linger: on iOS, the Clerk session's. Called from
/// background threads, so an implementation may block while it fetches one.
#[uniffi::export(with_foreign)]
pub trait TokenProvider: Send + Sync {
    /// A token that is valid now (refreshed first if it's about to expire), or `None` when signed
    /// out.
    fn token(&self) -> Option<String>;
    /// A new token after the server refused the current one, or `None`: sign in again.
    fn refresh(&self) -> Option<String>;
}

/// A [`TokenProvider`] as core's [`TokenSource`].
pub(crate) struct Tokens(pub(crate) Arc<dyn TokenProvider>);

impl TokenSource for Tokens {
    fn token(&self) -> anyhow::Result<String> {
        self.0.token().ok_or_else(|| anyhow::anyhow!("signed out"))
    }

    fn refresh(&self) -> anyhow::Result<String> {
        self.0.refresh().ok_or_else(|| anyhow::anyhow!("the session ended"))
    }
}

/// Told when study data changed, here or from another device, with the new generation.
#[uniffi::export(with_foreign)]
pub trait StudyObserver: Send + Sync {
    fn study_changed(&self, generation: u64);
}

/// Told when the sync status may have changed; read it with `LibrarySync::status`.
#[uniffi::export(with_foreign)]
pub trait SyncObserver: Send + Sync {
    fn sync_changed(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HighlightColor {
    Yellow,
    Green,
    Blue,
    Pink,
    Purple,
    Orange,
}

impl From<study::HighlightColor> for HighlightColor {
    fn from(c: study::HighlightColor) -> Self {
        match c {
            study::HighlightColor::Yellow => Self::Yellow,
            study::HighlightColor::Green => Self::Green,
            study::HighlightColor::Blue => Self::Blue,
            study::HighlightColor::Pink => Self::Pink,
            study::HighlightColor::Purple => Self::Purple,
            study::HighlightColor::Orange => Self::Orange,
        }
    }
}

impl From<HighlightColor> for study::HighlightColor {
    fn from(c: HighlightColor) -> Self {
        match c {
            HighlightColor::Yellow => Self::Yellow,
            HighlightColor::Green => Self::Green,
            HighlightColor::Blue => Self::Blue,
            HighlightColor::Pink => Self::Pink,
            HighlightColor::Purple => Self::Purple,
            HighlightColor::Orange => Self::Orange,
        }
    }
}

/// The six highlight colours, in the desktop's order (its keys 1–6).
#[uniffi::export]
pub fn highlight_colors() -> Vec<HighlightColor> {
    study::HighlightColor::ALL.into_iter().map(Into::into).collect()
}

/// A verse's own note.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Note {
    pub uid: String,
    pub body: String,
    /// Unix seconds.
    pub created: i64,
    pub updated: i64,
    /// The last change to it this session came from another device.
    pub edited_elsewhere: bool,
}

impl From<&study::Note> for Note {
    fn from(n: &study::Note) -> Self {
        Self {
            uid: n.uid.clone(),
            body: n.body.clone(),
            created: n.created,
            updated: n.updated,
            edited_elsewhere: n.edited_elsewhere,
        }
    }
}

/// What the reader marks on one verse.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VerseMarks {
    pub verse: u8,
    pub highlight: Option<HighlightColor>,
    pub note: bool,
    pub bookmark: bool,
}

/// A note in the notes list.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VerseNote {
    pub verse: VerseRef,
    /// "John 3:16".
    pub reference: String,
    pub note: Note,
}

/// What the sync footer says.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SyncState {
    SignedOut,
    /// Everything here is on the server, and the feed is caught up.
    Synced,
    /// Changes waiting to be sent, or the feed still catching up.
    Syncing {
        pending: u32,
    },
    /// The server can't be reached; this many changes are waiting.
    Offline {
        pending: u32,
    },
    /// Something was refused; `retry` tries again.
    Error {
        message: String,
    },
}

impl From<SyncStatus> for SyncState {
    fn from(s: SyncStatus) -> Self {
        match s {
            SyncStatus::SignedOut => Self::SignedOut,
            SyncStatus::Synced => Self::Synced,
            SyncStatus::Syncing(n) => Self::Syncing { pending: n as u32 },
            SyncStatus::Offline(n) => Self::Offline { pending: n as u32 },
            SyncStatus::Error(message) => Self::Error { message },
        }
    }
}

/// Highlights, bookmarks and notes, kept in `data_dir/study.sqlite`.
#[derive(uniffi::Object)]
pub struct Study {
    dir: PathBuf,
    store: Mutex<StudyStore>,
    observer: Option<Arc<dyn StudyObserver>>,
}

#[uniffi::export]
impl Study {
    #[uniffi::constructor]
    pub fn new(data_dir: String, observer: Option<Arc<dyn StudyObserver>>) -> Result<Arc<Self>, BibleError> {
        let dir = PathBuf::from(data_dir);
        let store = StudyStore::open(&dir.join("study.sqlite")).map_err(failed)?;
        Ok(Arc::new(Self { dir, store: Mutex::new(store), observer }))
    }

    /// Moves whenever study data changes.
    pub fn generation(&self) -> u64 {
        self.lock().generation()
    }

    /// The marks on a chapter's verses, for those that have any.
    pub fn chapter_marks(&self, book: u8, chapter: u8) -> Result<Vec<VerseMarks>, BibleError> {
        let c = chapter_idx(book, chapter)?;
        let s = self.lock();
        let mut marks = std::collections::BTreeMap::new();
        fn at(marks: &mut std::collections::BTreeMap<u8, VerseMarks>, v: CoreVerse) -> &mut VerseMarks {
            marks.entry(v.verse).or_insert(VerseMarks {
                verse: v.verse,
                highlight: None,
                note: false,
                bookmark: false,
            })
        }
        for (v, color) in s.highlights().into_iter().filter(|(v, _)| v.chapter_idx() == c) {
            at(&mut marks, v).highlight = Some(color.into());
        }
        for (v, _) in s.notes().into_iter().filter(|(v, _)| v.chapter_idx() == c) {
            at(&mut marks, v).note = true;
        }
        for (v, _) in s.bookmarks().into_iter().filter(|(v, _)| v.chapter_idx() == c) {
            at(&mut marks, v).bookmark = true;
        }
        Ok(marks.into_values().collect())
    }

    /// Highlights the verses in one colour, or clears them.
    pub fn set_highlight(
        &self,
        verses: Vec<VerseRef>,
        color: Option<HighlightColor>,
    ) -> Result<(), BibleError> {
        self.write(|s| {
            for v in &verses {
                s.set_highlight(core_verse(*v)?, color.map(Into::into)).map_err(failed)?;
            }
            Ok(())
        })
    }

    pub fn note(&self, verse: VerseRef) -> Result<Option<Note>, BibleError> {
        Ok(self.lock().note(core_verse(verse)?).map(Into::into))
    }

    /// Saves the verse's note; an empty (whitespace-only) body deletes it.
    pub fn set_note(&self, verse: VerseRef, body: String) -> Result<(), BibleError> {
        self.write(|s| s.set_note(core_verse(verse)?, &body).map_err(failed))
    }

    /// Bookmarks the verse, or removes its bookmark; returns whether it is now bookmarked.
    pub fn toggle_bookmark(&self, verse: VerseRef) -> Result<bool, BibleError> {
        let mut on = false;
        self.write(|s| {
            on = s.toggle_bookmark(core_verse(verse)?).map_err(failed)?;
            Ok(())
        })?;
        Ok(on)
    }

    /// Every verse note, in canonical order.
    pub fn notes(&self) -> Vec<VerseNote> {
        self.lock()
            .notes()
            .into_iter()
            .map(|(v, n)| VerseNote {
                verse: VerseRef { book: v.book.0, chapter: v.chapter, verse: v.verse },
                reference: v.display(),
                note: n.into(),
            })
            .collect()
    }

    /// Changes made here that the server hasn't confirmed yet.
    pub fn pending_changes(&self) -> u32 {
        self.lock().pending_changes() as u32
    }

    /// Forgets the library's cache, the outbox and the cursor, as a sign-out does.
    pub fn clear_library(&self) -> Result<(), BibleError> {
        self.write(|s| s.clear_library().map_err(failed))
    }
}

impl Study {
    fn lock(&self) -> MutexGuard<'_, StudyStore> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs a change, then tells the observer (outside the lock) if the generation moved.
    fn write(&self, f: impl FnOnce(&mut StudyStore) -> Result<(), BibleError>) -> Result<(), BibleError> {
        let (before, after, result) = {
            let mut s = self.lock();
            let before = s.generation();
            let result = f(&mut s);
            (before, s.generation(), result)
        };
        if after != before {
            self.notify(after);
        }
        result
    }

    /// Reads what the sync engine wrote, and tells the observer if anything changed.
    fn refresh(&self) {
        let changed = {
            let mut s = self.lock();
            s.refresh().ok().filter(|c| *c).map(|_| s.generation())
        };
        if let Some(generation) = changed {
            self.notify(generation);
        }
    }

    fn notify(&self, generation: u64) {
        if let Some(o) = &self.observer {
            o.study_changed(generation);
        }
    }
}

/// The sync engine for a [`Study`]. Dropping it stops syncing and keeps the cache and anything
/// still waiting.
#[derive(uniffi::Object)]
pub struct LibrarySync {
    study: Arc<Study>,
    handle: SyncHandle,
}

#[uniffi::export]
impl LibrarySync {
    /// Starts syncing the study with the Convex deployment at `convex_url` (production when
    /// `None`), signed in through `tokens`.
    #[uniffi::constructor]
    pub fn start(
        study: Arc<Study>,
        convex_url: Option<String>,
        tokens: Arc<dyn TokenProvider>,
        observer: Option<Arc<dyn SyncObserver>>,
    ) -> Result<Arc<Self>, BibleError> {
        let url = convex_url.unwrap_or_else(linger_core::api::convex_url);
        let backend = sync::ConvexBackend::new(&url).map_err(failed)?;
        Self::with_backend(study, Box::new(backend), tokens, observer)
    }

    pub fn status(&self) -> SyncState {
        self.handle.status().into()
    }

    /// Tries every waiting change again now.
    pub fn retry(&self) {
        self.handle.retry();
    }

    /// Waits up to `timeout_ms` until everything here is on the server and the feed is caught up;
    /// returns the status then. Blocks: call it off the main thread.
    pub fn wait_idle(&self, timeout_ms: u64) -> SyncState {
        match self.handle.wait_idle(Duration::from_millis(timeout_ms)) {
            Ok(_) => SyncState::Synced,
            Err(s) => s.into(),
        }
    }
}

impl LibrarySync {
    pub(crate) fn with_backend(
        study: Arc<Study>,
        backend: Box<dyn Backend>,
        tokens: Arc<dyn TokenProvider>,
        observer: Option<Arc<dyn SyncObserver>>,
    ) -> Result<Arc<Self>, BibleError> {
        let handle = sync::start(&study.dir, backend, Arc::new(Tokens(tokens))).map_err(failed)?;
        let s = study.clone();
        // On the engine's thread: read in what it wrote, then say the status may have moved.
        handle.on_change(move || {
            s.refresh();
            if let Some(o) = &observer {
                o.sync_changed();
            }
        });
        study.lock().on_outbox(handle.notifier());
        Ok(Arc::new(Self { study, handle }))
    }
}

impl Drop for LibrarySync {
    fn drop(&mut self) {
        self.study.lock().on_outbox(|| {});
    }
}

pub(crate) fn failed(e: impl std::fmt::Display) -> BibleError {
    BibleError::Failed { message: format!("{e:#}") }
}

fn core_verse(v: VerseRef) -> Result<CoreVerse, BibleError> {
    let c = chapter_idx(v.book, v.chapter)?;
    if v.verse == 0 {
        return Err(BibleError::InvalidChapter { book: v.book, chapter: v.chapter });
    }
    Ok(CoreVerse::new(c.book(), c.chapter(), v.verse))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::Instant;

    use linger_core::sync::FakeServer;

    use super::*;

    struct Provider(Option<String>, AtomicUsize);

    impl TokenProvider for Provider {
        fn token(&self) -> Option<String> {
            self.1.fetch_add(1, Ordering::Relaxed);
            self.0.clone()
        }
        fn refresh(&self) -> Option<String> {
            None
        }
    }

    #[derive(Default)]
    struct Seen(AtomicU64, AtomicUsize);

    impl StudyObserver for Seen {
        fn study_changed(&self, generation: u64) {
            self.0.store(generation, Ordering::Relaxed);
        }
    }

    impl SyncObserver for Seen {
        fn sync_changed(&self) {
            self.1.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct Device {
        dir: PathBuf,
        study: Arc<Study>,
        seen: Arc<Seen>,
        provider: Arc<Provider>,
        sync: Option<Arc<LibrarySync>>,
    }

    impl Device {
        fn new(name: &str, server: &Arc<FakeServer>) -> Self {
            let dir = std::env::temp_dir().join(format!("linger-ffi-sync-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let seen = Arc::new(Seen::default());
            let study = Study::new(dir.to_string_lossy().into_owned(), Some(seen.clone())).unwrap();
            let provider = Arc::new(Provider(Some("t".into()), AtomicUsize::new(0)));
            let sync = LibrarySync::with_backend(
                study.clone(),
                Box::new(server.backend()),
                provider.clone(),
                Some(seen.clone()),
            )
            .unwrap();
            Self { dir, study, seen, provider, sync: Some(sync) }
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            self.sync = None;
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn eventually(what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    const ROM_8_28: VerseRef = VerseRef { book: 44, chapter: 8, verse: 28 };

    #[test]
    fn a_note_and_highlight_reach_the_other_device_and_its_observer() {
        let server = FakeServer::new();
        let (a, b) = (Device::new("a", &server), Device::new("b", &server));
        let before = b.seen.0.load(Ordering::Relaxed);

        a.study.set_note(ROM_8_28, "All things".into()).unwrap();
        a.study
            .set_highlight(vec![ROM_8_28, VerseRef { verse: 29, ..ROM_8_28 }], Some(HighlightColor::Purple))
            .unwrap();
        // The writer's own observer hears its writes at once.
        assert_eq!(a.seen.0.load(Ordering::Relaxed), a.study.generation());

        eventually("the note on b", || {
            b.study.note(ROM_8_28).unwrap().is_some_and(|n| n.body == "All things")
        });
        eventually("both highlights on b", || b.study.chapter_marks(44, 8).unwrap().len() == 2);
        assert!(b.seen.0.load(Ordering::Relaxed) > before, "b's study observer wasn't told");
        assert!(b.seen.1.load(Ordering::Relaxed) > 0, "b's sync observer wasn't told");
        assert!(b.provider.1.load(Ordering::Relaxed) > 0, "sync didn't ask for a token");
        assert_eq!(
            b.study.chapter_marks(44, 8).unwrap(),
            [
                VerseMarks {
                    verse: 28,
                    highlight: Some(HighlightColor::Purple),
                    note: true,
                    bookmark: false
                },
                VerseMarks {
                    verse: 29,
                    highlight: Some(HighlightColor::Purple),
                    note: false,
                    bookmark: false
                },
            ]
        );
        assert_eq!(b.study.notes()[0].reference, "Romans 8:28");
        assert_eq!(a.sync.as_ref().unwrap().wait_idle(5_000), SyncState::Synced);
        assert_eq!(a.study.pending_changes(), 0);
    }

    #[test]
    fn a_change_made_while_stopped_goes_when_sync_starts_again() {
        let server = FakeServer::new();
        let mut a = Device::new("stopped", &server);
        a.sync = None;
        assert!(a.study.toggle_bookmark(ROM_8_28).unwrap());
        assert_eq!(a.study.pending_changes(), 1);
        assert_eq!(server.bookmark(linger_core::records::VerseId::from(core_verse(ROM_8_28).unwrap())), None);

        let provider = Arc::new(Provider(Some("t".into()), AtomicUsize::new(0)));
        let sync =
            LibrarySync::with_backend(a.study.clone(), Box::new(server.backend()), provider, None).unwrap();
        assert_eq!(sync.wait_idle(5_000), SyncState::Synced);
        assert!(
            server.bookmark(linger_core::records::VerseId::from(core_verse(ROM_8_28).unwrap())).is_some()
        );
    }

    #[test]
    fn rejects_verses_outside_the_canon() {
        let server = FakeServer::new();
        let a = Device::new("invalid", &server);
        assert!(a.study.set_note(VerseRef { book: 66, chapter: 1, verse: 1 }, "x".into()).is_err());
        assert!(a.study.set_note(VerseRef { book: 42, chapter: 22, verse: 1 }, "x".into()).is_err());
        assert!(a.study.set_note(VerseRef { book: 42, chapter: 3, verse: 0 }, "x".into()).is_err());
        assert!(a.study.notes().is_empty());
    }
}
