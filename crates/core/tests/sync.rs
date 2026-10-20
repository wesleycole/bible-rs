//! The sync engine against an in-memory server: what a person sees on two devices.
#![cfg(feature = "sync")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use linger_core::auth::StaticToken;
use linger_core::books::{BookId, VerseRef};
use linger_core::records::VerseId;
use linger_core::study::{HighlightColor, StudyStore};
use linger_core::sync::{self, FakeServer, SyncHandle, SyncStatus};

struct Device {
    dir: PathBuf,
    store: StudyStore,
    sync: Option<SyncHandle>,
}

impl Device {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("linger-sync-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self::open(dir)
    }

    fn open(dir: PathBuf) -> Self {
        let store = StudyStore::open(&dir.join("study.sqlite")).unwrap();
        Self { dir, store, sync: None }
    }

    fn start(&mut self, server: &Arc<FakeServer>) {
        let handle =
            sync::start(&self.dir, Box::new(server.backend()), Arc::new(StaticToken("t".into()))).unwrap();
        self.store.on_outbox(handle.notifier());
        self.sync = Some(handle);
    }

    fn stop(&mut self) {
        self.sync = None;
        self.store.on_outbox(|| {});
    }

    /// Waits until the device has sent everything and read the feed up to the server's `seq`.
    fn settle(&mut self, server: &FakeServer) {
        let sync = self.sync.as_ref().unwrap();
        // The outbox first: until the engine sees a new change its status still says Synced.
        eventually(|| self.store.pending_changes() == 0);
        eventually(|| sync.status() == SyncStatus::Synced && sync.cursor() >= server.seq());
        self.store.refresh().unwrap();
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.sync = None;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn eventually(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn verse(chapter: u8, v: u8) -> VerseRef {
    VerseRef::new(BookId(42), chapter, v)
}

#[test]
fn a_highlight_made_on_one_device_appears_on_the_other_and_a_clear_follows() {
    let server = FakeServer::new();
    let (mut a, mut b) = (Device::new("a1"), Device::new("b1"));
    a.start(&server);
    b.start(&server);
    a.store.set_highlight(verse(3, 16), Some(HighlightColor::Pink)).unwrap();
    a.store.set_bookmark(verse(1, 1), Some("Beginning")).unwrap();
    a.settle(&server);
    let generation = b.store.generation();
    b.settle(&server);
    assert_eq!(b.store.highlight(verse(3, 16)), Some(HighlightColor::Pink));
    assert_eq!(b.store.bookmark(verse(1, 1)).unwrap().label, "Beginning");
    assert!(b.store.generation() > generation, "the feed bumps generation");

    a.store.set_highlight(verse(3, 16), None).unwrap();
    a.settle(&server);
    b.settle(&server);
    assert_eq!(b.store.highlight(verse(3, 16)), None, "the tombstone reaches the other device");
    assert!(server.highlight(VerseId::from(verse(3, 16))).unwrap().2, "a tombstone on the server");
}

#[test]
fn changes_made_offline_survive_a_restart_and_go_in_order() {
    let server = FakeServer::new();
    server.set_page_records(2);
    let mut a = Device::new("a2");
    a.start(&server);
    a.settle(&server);
    server.set_offline(true);
    for (i, c) in
        [HighlightColor::Yellow, HighlightColor::Green, HighlightColor::Blue].into_iter().enumerate()
    {
        a.store.set_highlight(verse(1, i as u8 + 1), Some(c)).unwrap();
    }
    a.store.set_highlight(verse(1, 1), Some(HighlightColor::Orange)).unwrap();
    let sync = a.sync.as_ref().unwrap();
    eventually(|| matches!(sync.status(), SyncStatus::Offline(3)));
    a.stop();
    let dir = a.dir.clone();
    drop(std::mem::replace(&mut a.store, StudyStore::open_in_memory().unwrap()));
    a.store = StudyStore::open(&dir.join("study.sqlite")).unwrap();
    assert_eq!(a.store.pending_changes(), 3, "the outbox survives a restart");

    server.set_offline(false);
    a.start(&server);
    a.settle(&server);
    assert_eq!(a.store.pending_changes(), 0);
    // Sent oldest first: verse 1 (its later edit replaced the first in place), then 2, then 3.
    let seqs: Vec<u64> = (1..=3).map(|v| server.highlight(VerseId::from(verse(1, v))).unwrap().0).collect();
    assert_eq!(seqs, [1, 2, 3]);
    assert_eq!(server.highlight(VerseId::from(verse(1, 1))).unwrap().1, "orange");

    // A second device pulls all of it across several pages.
    let mut b = Device::new("b2");
    b.start(&server);
    b.settle(&server);
    assert_eq!(b.store.highlights().len(), 3);
}

#[test]
fn a_record_keeps_its_exact_date_on_another_device_after_an_idle_gap() {
    let server = FakeServer::new();
    let mut a = Device::new("clock-a");
    a.start(&server);
    a.settle(&server);
    a.store.set_highlight(verse(5, 5), Some(HighlightColor::Purple)).unwrap();
    a.settle(&server);
    // Nobody writes for a while, so the feed's cached serverTime falls behind the real clock.
    std::thread::sleep(Duration::from_millis(2_500));
    let mut b = Device::new("clock-b");
    b.start(&server);
    b.settle(&server);
    let dated = |d: &Device| d.store.highlights_dated().into_iter().find(|h| h.0 == verse(5, 5)).unwrap().2;
    assert_eq!(dated(&b), dated(&a));
}

#[test]
fn a_wrong_stored_clock_offset_never_dates_what_the_feed_brings() {
    let server = FakeServer::new();
    let mut a = Device::new("skew-a");
    a.start(&server);
    a.settle(&server);
    a.store.set_highlight(verse(7, 7), Some(HighlightColor::Green)).unwrap();
    a.settle(&server);
    // B's last connection measured an offset that is now wrong (another network, a clock fixed).
    let b_dir = std::env::temp_dir().join(format!("linger-sync-{}-skew-b", std::process::id()));
    let _ = std::fs::remove_dir_all(&b_dir);
    let mut b = Device::open(b_dir.clone());
    rusqlite::Connection::open(b_dir.join("study.sqlite"))
        .unwrap()
        .execute("INSERT OR REPLACE INTO sync_state(key, value) VALUES('clock_offset_ms', -5000)", [])
        .unwrap();
    b.start(&server);
    b.settle(&server);
    let dated = |d: &Device| d.store.highlights_dated().into_iter().find(|h| h.0 == verse(7, 7)).unwrap().2;
    assert_eq!(dated(&b), dated(&a));
}

#[test]
fn a_stale_edit_becomes_a_conflict_and_the_newer_one_stands() {
    let server = FakeServer::new();
    let (mut a, mut b) = (Device::new("a3"), Device::new("b3"));
    a.start(&server);
    b.start(&server);
    a.store.set_highlight(verse(3, 16), Some(HighlightColor::Yellow)).unwrap();
    a.settle(&server);
    b.settle(&server);

    // B goes quiet; A changes the colour; B, still on yellow, changes it too.
    b.stop();
    a.store.set_highlight(verse(3, 16), Some(HighlightColor::Blue)).unwrap();
    a.settle(&server);
    b.store.set_highlight(verse(3, 16), Some(HighlightColor::Pink)).unwrap();
    b.start(&server);
    b.settle(&server);

    assert_eq!(server.highlight(VerseId::from(verse(3, 16))).unwrap().1, "blue");
    assert_eq!(b.store.highlight(verse(3, 16)), Some(HighlightColor::Blue), "B shows the copy that stood");
    assert_eq!(b.store.pending_changes(), 0, "a conflict answer is done");
    let conflicts = b.store.conflicts();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        (conflicts[0].verse, conflicts[0].mine.as_str(), conflicts[0].theirs.as_str()),
        (verse(3, 16), "\"pink\"", "\"blue\"")
    );
    assert!(a.store.conflicts().is_empty());
}

#[test]
fn a_cursor_behind_purged_tombstones_resyncs_and_drops_what_was_deleted() {
    let server = FakeServer::new();
    let (mut a, mut b) = (Device::new("a4"), Device::new("b4"));
    a.start(&server);
    b.start(&server);
    a.store.set_highlight(verse(1, 1), Some(HighlightColor::Green)).unwrap();
    a.store.set_bookmark(verse(1, 2), Some("")).unwrap();
    a.settle(&server);
    b.settle(&server);
    b.stop();
    // While B is away the highlight is deleted, something new is made, and the tombstone purged.
    a.store.set_highlight(verse(1, 1), None).unwrap();
    a.store.set_highlight(verse(2, 1), Some(HighlightColor::Purple)).unwrap();
    a.settle(&server);
    server.purge_tombstones();
    b.start(&server);
    b.settle(&server);
    assert_eq!(b.store.highlight(verse(1, 1)), None, "resync forgot the deleted highlight");
    assert_eq!(b.store.highlight(verse(2, 1)), Some(HighlightColor::Purple));
    assert!(b.store.is_bookmarked(verse(1, 2)));
}

#[test]
fn a_refused_change_shows_the_error_and_retry_sends_it() {
    let server = FakeServer::new();
    let mut a = Device::new("a5");
    a.start(&server);
    a.settle(&server);
    server.fail_next(sync::CallError::Refused("the library is full".into()));
    a.store.set_bookmark(verse(1, 1), Some("x")).unwrap();
    let sync = a.sync.as_ref().unwrap();
    eventually(|| sync.status() != SyncStatus::Synced);
    let started = Instant::now();
    assert_eq!(sync.wait_idle(Duration::from_secs(30)), Err(SyncStatus::Error("the library is full".into())));
    assert!(started.elapsed() < Duration::from_secs(5), "an error ends the wait at once");
    sync.retry();
    a.settle(&server);
    assert_eq!(server.bookmark(VerseId::from(verse(1, 1))).unwrap().1, "x");
}

/// study.sqlite as every release before M1 wrote it.
fn old_file(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE highlights(book TEXT NOT NULL, chapter INTEGER NOT NULL, verse INTEGER NOT NULL,
             color INTEGER NOT NULL, created INTEGER NOT NULL, PRIMARY KEY(book, chapter, verse));
         CREATE TABLE notes(book TEXT NOT NULL, chapter INTEGER NOT NULL, verse INTEGER NOT NULL,
             body TEXT NOT NULL, created INTEGER NOT NULL, updated INTEGER NOT NULL, PRIMARY KEY(book, chapter, verse));
         CREATE TABLE bookmarks(book TEXT NOT NULL, chapter INTEGER NOT NULL, verse INTEGER NOT NULL,
             label TEXT NOT NULL, created INTEGER NOT NULL, PRIMARY KEY(book, chapter, verse));
         CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
         INSERT INTO highlights VALUES('JHN', 3, 16, 3, 1700000000), ('PSA', 23, 1, 0, 1700000100);
         INSERT INTO bookmarks VALUES('GEN', 1, 1, 'Start', 1700000200);
         INSERT INTO notes VALUES('JHN', 3, 16, 'Love', 1700000000, 1700000000);",
    )
    .unwrap();
}

#[test]
fn an_existing_library_migrates_keeps_everything_and_uploads_once() {
    let dir = std::env::temp_dir().join(format!("linger-sync-{}-migrate", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    old_file(&dir.join("study.sqlite"));
    let mut a = Device::open(dir);
    let jn316 = verse(3, 16);
    let ps231 = VerseRef::new(BookId(18), 23, 1);
    let gen11 = VerseRef::new(BookId(0), 1, 1);
    assert_eq!(a.store.highlight(jn316), Some(HighlightColor::Pink));
    assert_eq!(a.store.highlight(ps231), Some(HighlightColor::Yellow));
    assert_eq!(a.store.highlights_dated()[0].2, 1_700_000_100, "creation times survive");
    assert_eq!(a.store.bookmark(gen11).unwrap().label, "Start");
    assert_eq!(a.store.note(jn316).unwrap().body, "Love");
    assert_eq!(a.store.note(jn316).unwrap().created, 1_700_000_000);
    assert_eq!(a.store.pending_changes(), 4);

    let server = FakeServer::new();
    a.start(&server);
    a.settle(&server);
    assert_eq!(server.seq(), 4);
    assert_eq!(server.notes()[0].text.as_deref(), Some("Love"));
    assert_eq!(server.highlight(VerseId(42_003_016)).unwrap().1, "pink");
    assert_eq!(server.bookmark(VerseId(1_001)).unwrap().1, "Start");
    // A second device with its own library: the verse the server already has keeps the server's
    // copy, and since the colours differ the person is told; its other highlight goes up.
    let dir = std::env::temp_dir().join(format!("linger-sync-{}-migrate-b", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    old_file(&dir.join("study.sqlite"));
    let conn = rusqlite::Connection::open(dir.join("study.sqlite")).unwrap();
    conn.execute_batch(
        "UPDATE highlights SET color = 1 WHERE book = 'JHN'; DELETE FROM bookmarks;
         INSERT INTO highlights VALUES('MRK', 1, 1, 2, 1700000300);",
    )
    .unwrap();
    drop(conn);
    let mut b = Device::open(dir);
    b.start(&server);
    b.settle(&server);
    assert_eq!(b.store.highlight(jn316), Some(HighlightColor::Pink), "the server's copy stands");
    let conflicts = b.store.conflicts();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        (conflicts[0].verse, conflicts[0].mine.as_str(), conflicts[0].theirs.as_str()),
        (jn316, "\"green\"", "\"pink\"")
    );
    assert_eq!(server.seq(), 5, "only Mark 1:1 was new: the same note on the same verse isn't sent again");
    assert_eq!(server.notes().len(), 1);
    assert_eq!(b.store.note(jn316).unwrap().body, "Love");
    // Creation times cross devices: the bookmark B pulled and the highlight it lost both carry
    // the dates A's library had, not the moment B saw them.
    assert_eq!(b.store.bookmark(gen11).unwrap().created, 1_700_000_200);
    let dated =
        |d: &Device, v: VerseRef| d.store.highlights_dated().into_iter().find(|h| h.0 == v).unwrap().2;
    assert_eq!(dated(&b, jn316), dated(&a, jn316));
    assert_eq!(server.highlight(VerseId(40_001_001)).unwrap().1, "blue");
    // Opening again doesn't migrate or upload again.
    a.stop();
    let dir = a.dir.clone();
    a.store = StudyStore::open(&dir.join("study.sqlite")).unwrap();
    assert_eq!(a.store.pending_changes(), 0);
    assert_eq!(a.store.highlight(jn316), Some(HighlightColor::Pink));
}

#[test]
fn a_note_syncs_and_a_stale_edit_is_kept_beside_the_newer_one() {
    let server = FakeServer::new();
    let (mut a, mut b) = (Device::new("note-a"), Device::new("note-b"));
    a.start(&server);
    b.start(&server);
    a.store.set_note(verse(3, 16), "Love").unwrap();
    a.settle(&server);
    b.settle(&server);
    assert_eq!(b.store.note(verse(3, 16)).unwrap().body, "Love");
    assert_eq!(b.store.note(verse(3, 16)).unwrap().created, a.store.note(verse(3, 16)).unwrap().created);

    // B goes quiet; A edits; B, still on the first text, edits too.
    b.stop();
    a.store.set_note(verse(3, 16), "Love, endlessly").unwrap();
    a.settle(&server);
    b.store.set_note(verse(3, 16), "God's love").unwrap();
    b.start(&server);
    b.settle(&server);
    a.settle(&server);

    // A's later edit is the note on the verse; B's is kept as a copy of it, on both devices' server.
    let notes = server.notes();
    let original = notes.iter().find(|n| n.conflict_of.is_none()).unwrap();
    let copy = notes.iter().find(|n| n.conflict_of.is_some()).unwrap();
    assert_eq!(original.text.as_deref(), Some("Love, endlessly"));
    assert_eq!(
        (copy.text.as_deref(), copy.conflict_of.as_deref()),
        (Some("God's love"), Some(original.uid.as_str()))
    );
    for d in [&a, &b] {
        assert_eq!(d.store.note(verse(3, 16)).unwrap().body, "Love, endlessly");
    }
    let conflicts = b.store.conflicts();
    assert_eq!(
        (conflicts[0].kind.as_str(), conflicts[0].verse, conflicts[0].mine.as_str()),
        ("note", verse(3, 16), "\"God's love\"")
    );
    assert_eq!(b.store.pending_changes(), 0);

    // Both notes are in the backup, the copy naming its original.
    let export: serde_json::Value =
        serde_json::from_str(&b.store.export_json("KJV", |_| None).unwrap()).unwrap();
    let notes = export["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes.iter().any(|n| n["note"] == "God's love" && n["conflict_of"] == original.uid.as_str()));

    // A deletes the original: the copy is now an ordinary note, the verse's own, on both devices.
    a.store.set_note(verse(3, 16), "").unwrap();
    a.settle(&server);
    b.settle(&server);
    for d in [&a, &b] {
        assert_eq!(d.store.note(verse(3, 16)).unwrap().body, "God's love");
    }
    // And a backup imported elsewhere brings both notes back as two, not one over the other.
    let mut c = StudyStore::open_in_memory().unwrap();
    c.import_json(&serde_json::to_string(&export).unwrap()).unwrap();
    let again: serde_json::Value = serde_json::from_str(&c.export_json("KJV", |_| None).unwrap()).unwrap();
    assert_eq!(again["notes"].as_array().unwrap().len(), 2);
    assert_eq!(c.note(verse(3, 16)).unwrap().body, "Love, endlessly");
}

#[test]
fn a_conversation_saved_on_one_device_opens_on_the_other() {
    use linger_core::chat::Message;
    let server = FakeServer::new();
    let (mut a, mut b) = (Device::new("chat-a"), Device::new("chat-b"));
    a.start(&server);
    b.start(&server);
    let first = [Message::user("Who was Nicodemus?")];
    let id = a.store.save_chat(None, "Nicodemus", &first, "[1]").unwrap();
    a.settle(&server);
    let messages = [Message::user("Who was Nicodemus?"), Message::assistant("A Pharisee {{John 3:1}}.")];
    a.store.save_chat(Some(id), "Nicodemus", &messages, "[1,2]").unwrap();
    a.settle(&server);
    b.settle(&server);
    let chats = b.store.chats();
    assert_eq!(chats.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Nicodemus"]);
    let saved = b.store.chat(chats[0].id).expect("the conversation was fetched");
    assert_eq!((saved.messages.as_slice(), saved.api.as_str()), (&messages[..], "[1,2]"));
    assert_eq!(server.chats().len(), 1, "continuing a conversation saves the same chat");

    b.store.delete_chat(chats[0].id).unwrap();
    b.settle(&server);
    a.settle(&server);
    assert!(a.store.chats().is_empty());
    assert!(server.chats().is_empty());
}

#[test]
fn a_record_type_newer_than_this_build_stops_sync_and_says_to_update() {
    let server = FakeServer::new();
    let mut a = Device::new("newer");
    a.start(&server);
    a.settle(&server);
    server.deliver(serde_json::json!({
        "records": [{ "type": "photo", "uid": "p1", "seq": 1 }], "seq": 1, "serverTime": 1
    }));
    let sync = a.sync.as_ref().unwrap();
    eventually(|| matches!(sync.status(), SyncStatus::Error(_)));
    let SyncStatus::Error(e) = sync.status() else { unreachable!() };
    assert!(e.starts_with("Update Linger to keep syncing"), "{e}");
    assert_eq!(sync.cursor(), 0, "the cursor doesn't move past what it couldn't read");
}
