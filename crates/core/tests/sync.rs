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
        self.start_as(server, "t");
    }

    fn start_as(&mut self, server: &Arc<FakeServer>, token: &str) {
        let handle =
            sync::start(&self.dir, Box::new(server.backend()), Arc::new(StaticToken(token.into()))).unwrap();
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

#[test]
fn a_feed_the_server_refused_resumes_without_retry() {
    let server = FakeServer::new();
    let (mut a, mut b) = (Device::new("a14"), Device::new("b14"));
    a.start(&server);
    b.start(&server);
    a.settle(&server);
    b.settle(&server);
    server.fail_feed("Server Error");
    let sync = b.sync.as_ref().unwrap();
    eventually(|| sync.status() == SyncStatus::Error("Server Error".into()));
    a.store.set_highlight(verse(4, 7), Some(HighlightColor::Blue)).unwrap();
    a.settle(&server);
    b.settle(&server);
    assert_eq!(b.store.highlight(verse(4, 7)), Some(HighlightColor::Blue));
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
        // The copy is shown beside the note it collided with, on both devices.
        let copies: Vec<&str> =
            d.store.conflict_copies(verse(3, 16)).iter().map(|n| n.body.as_str()).collect();
        assert_eq!(copies, ["God's love"]);
    }
    // B's note was replaced by A's edit: B's notes page says it was edited on another device.
    // A wrote what stands, so its own doesn't.
    assert!(b.store.note(verse(3, 16)).unwrap().edited_elsewhere);
    assert!(!a.store.note(verse(3, 16)).unwrap().edited_elsewhere);
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

    // Editing it here again is this device's edit.
    b.store.set_note(verse(3, 16), "Love, endlessly.").unwrap();
    assert!(!b.store.note(verse(3, 16)).unwrap().edited_elsewhere);
    b.settle(&server);
    a.settle(&server);
    assert!(a.store.note(verse(3, 16)).unwrap().edited_elsewhere);

    // A deletes the original: the copy is now an ordinary note, the verse's own, on both devices.
    a.store.set_note(verse(3, 16), "").unwrap();
    a.settle(&server);
    b.settle(&server);
    for d in [&a, &b] {
        assert_eq!(d.store.note(verse(3, 16)).unwrap().body, "God's love");
        assert!(d.store.conflict_copies(verse(3, 16)).is_empty());
    }
    // And a backup imported elsewhere brings both notes back as two, not one over the other.
    let mut c = StudyStore::open_in_memory().unwrap();
    c.import_json(&serde_json::to_string(&export).unwrap()).unwrap();
    let again: serde_json::Value = serde_json::from_str(&c.export_json("KJV", |_| None).unwrap()).unwrap();
    assert_eq!(again["notes"].as_array().unwrap().len(), 2);
    assert_eq!(c.note(verse(3, 16)).unwrap().body, "Love, endlessly");
}

/// The notes a device's backup lists, by uid, with their text and times to the second.
fn exported_notes(d: &Device) -> Vec<serde_json::Value> {
    let export: serde_json::Value =
        serde_json::from_str(&d.store.export_json("KJV", |_| None).unwrap()).unwrap();
    let mut notes = export["notes"].as_array().unwrap().clone();
    notes.sort_by_key(|n| n["uid"].to_string());
    notes
}

#[test]
fn the_writer_holds_the_same_note_as_every_other_device_times_included() {
    let server = FakeServer::new();
    let (mut a, mut b) = (Device::new("same-a"), Device::new("same-b"));
    a.start(&server);
    b.start(&server);
    a.store.set_note(verse(3, 17), "Not to condemn (Rom 8:1; Romans 5)").unwrap();
    a.settle(&server);
    b.settle(&server);
    // The references written in it are anchors too, beside the verse it is on.
    let anchors: Vec<(u32, u32, String)> =
        server.notes()[0].anchors.iter().map(|a| (a.from.0, a.to.0, format!("{:?}", a.origin))).collect();
    assert_eq!(
        anchors,
        [
            (42_003_017, 42_003_017, "User".into()),
            (44_008_001, 44_008_001, "Typed".into()),
            (44_005_001, 44_005_021, "Typed".into())
        ]
    );

    // An edit made offline reaches the server a second later, which stamps the edit time.
    a.stop();
    a.store.set_note(verse(3, 17), "Not to condemn, but to save").unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    a.start(&server);
    a.settle(&server);
    b.settle(&server);
    assert_eq!(exported_notes(&a), exported_notes(&b));

    // A stale edit becomes a conflict copy, made by the server a second after the edit.
    b.stop();
    a.store.set_note(verse(3, 17), "That the world might be saved").unwrap();
    a.settle(&server);
    b.store.set_note(verse(3, 17), "Through him (stale)").unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    b.start(&server);
    b.settle(&server);
    a.settle(&server);
    let notes = exported_notes(&a);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!(notes, exported_notes(&b));

    // Saving the same text again changes nothing, here or anywhere.
    std::thread::sleep(Duration::from_millis(1100));
    a.store.set_note(verse(3, 17), "That the world might be saved").unwrap();
    assert_eq!(a.store.pending_changes(), 0);
    assert_eq!(exported_notes(&a), notes);
}

#[test]
fn a_slide_attached_to_a_sermon_on_one_device_is_attached_on_the_other() {
    use linger_core::records::{Item, ItemDetails, ItemKind, ItemStatus};
    let server = FakeServer::new();
    let item = |uid: &str, kind| Item {
        uid: uid.into(),
        seq: 0,
        deleted_at: None,
        kind,
        title: String::new(),
        status: ItemStatus::Ready,
        text: None,
        text_file_uid: None,
        excerpt: None,
        summary: None,
        outline: None,
        source_uid: None,
        sermon_uid: None,
        offset_ms: None,
        recorded_on: None,
        details: None,
        conflict_of: None,
        captured_at: 1_800_000_000_000,
        updated_at: None,
        unreviewed: false,
        anchors: Vec::new(),
        files: Vec::new(),
    };
    // Captured on a phone, which this build doesn't make yet.
    server.add_item(item("sermon", ItemKind::Sermon));
    server.add_item(item("slide", ItemKind::Slide));
    let (mut a, mut b) = (Device::new("details-a"), Device::new("details-b"));
    a.start(&server);
    b.start(&server);
    a.settle(&server);
    let details = ItemDetails {
        title: Some("Romans 8".into()),
        speaker: Some("A. Preacher".into()),
        from: None,
        started_at: 1_800_000_000_000,
        ended_at: None,
    };
    a.store.set_item_details("sermon", "Romans 8", Some(details.clone()), None).unwrap();
    a.store.set_item_details("slide", "Slide 3", None, Some(("sermon".into(), Some(90_000)))).unwrap();
    a.settle(&server);
    b.settle(&server);
    let items = server.notes();
    let get = |uid: &str| items.iter().find(|i| i.uid == uid).unwrap().clone();
    assert_eq!((get("sermon").title, get("sermon").details), ("Romans 8".to_string(), Some(details)));
    assert_eq!((get("slide").sermon_uid.as_deref(), get("slide").offset_ms), (Some("sermon"), Some(90_000)));
    // Neither is a note, on either device.
    assert!(a.store.notes().is_empty() && b.store.notes().is_empty());
    assert_eq!(b.store.pending_changes(), 0);
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

/// Unsigned tokens for two accounts: only their `sub` matters here.
const USER_1: &str = "h.eyJzdWIiOiJ1c2VyXzEifQ.s";
const USER_2: &str = "h.eyJzdWIiOiJ1c2VyXzIifQ.s";

#[test]
fn another_account_signing_in_never_sees_the_last_ones_library() {
    // Each account has its own library on the server; the device keeps one cache.
    let (first, second) = (FakeServer::new(), FakeServer::new());
    let mut a = Device::new("owner");
    a.start_as(&first, USER_1);
    a.store.set_highlight(verse(3, 16), Some(HighlightColor::Pink)).unwrap();
    a.store.set_note(verse(3, 17), "Not to condemn").unwrap();
    a.settle(&first);
    assert!(a.sync.as_ref().unwrap().cursor() > 0);

    // The same account again (a restart): the cache stays.
    a.stop();
    a.start_as(&first, USER_1);
    a.settle(&first);
    assert_eq!(a.store.highlight(verse(3, 16)), Some(HighlightColor::Pink));

    // The session ended without a sign-out, and someone else signs in.
    a.stop();
    a.start_as(&second, USER_2);
    a.settle(&second);
    assert_eq!(a.sync.as_ref().unwrap().cursor(), 0, "the first account's cursor is gone");
    assert_eq!(a.store.counts(), (0, 0, 0), "the first account's library is gone");
    assert!(second.notes().is_empty() && second.highlight(VerseId::from(verse(3, 16))).is_none());

    // The second account's own writes go to its library alone.
    a.store.set_highlight(verse(1, 1), Some(HighlightColor::Green)).unwrap();
    a.settle(&second);
    assert!(second.highlight(VerseId::from(verse(1, 1))).is_some());
    assert!(first.highlight(VerseId::from(verse(1, 1))).is_none());
}

/// Records `seconds` of a moving tone in `data`, queueing the recording for the server as a
/// recorder does: at the start, as each part closes, and at the end.
fn record(device: &mut Device, seconds: u32) -> (PathBuf, linger_core::recording::Manifest) {
    use linger_core::recording::{ChunkWriter, SAMPLE_RATE, Source};
    let mut w = ChunkWriter::create(&device.dir, Source::Microphone).unwrap();
    let dir = w.dir().to_path_buf();
    let queue = |device: &mut Device| {
        let m = linger_core::recording::read(&dir).unwrap();
        device.store.queue_recording(&dir, &m).unwrap();
    };
    queue(device);
    let pcm: Vec<i16> = (0..SAMPLE_RATE * seconds)
        .map(|i| {
            let t = f64::from(i) / f64::from(SAMPLE_RATE);
            ((t * (300.0 + 50.0 * (t * 0.7).sin()) * std::f64::consts::TAU).sin() * 6000.0) as i16
        })
        .collect();
    let mut parts = 0;
    for block in pcm.chunks(SAMPLE_RATE as usize / 10) {
        w.push(block).unwrap();
        let m = linger_core::recording::read(&dir).unwrap();
        let closed = m.aac.as_ref().map_or(0, |a| a.files.len());
        if closed != parts {
            parts = closed;
            queue(device);
        }
    }
    let m = w.finish().unwrap();
    device.store.queue_recording(&dir, &m).unwrap();
    // Queueing again adds nothing.
    device.store.queue_recording(&dir, &m).unwrap();
    (dir, m)
}

#[test]
#[cfg(all(feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
fn a_pro_recording_uploads_its_parts_then_one_m4a() {
    let server = FakeServer::new();
    let mut a = Device::new("sermon-pro");
    a.start(&server);
    let (dir, m) = record(&mut a, 5 * 60 + 20);
    a.settle(&server);

    let parts = m.aac.as_ref().unwrap();
    let on_disk: Vec<u8> =
        parts.files.iter().flat_map(|p| std::fs::read(dir.join(&p.file)).unwrap()).collect();
    assert_eq!(server.joined_files(&m.id, "audioPart"), (2, on_disk), "both ADTS parts, in order");
    let m4a = std::fs::read(dir.join("audio.m4a")).unwrap();
    assert_eq!(server.joined_files(&m.id, "uploadPart"), (1, m4a), "the M4A, in slices");
    // The fake's finish checked the slices against audioFileUid.
    let finished = server.finished_sermons();
    assert_eq!(finished.len(), 1);
    assert!(finished[0]["audioFileUid"].is_string());
    assert_eq!(finished[0]["durationMs"], (5 * 60 + 20) * 1000);
    assert_eq!(a.store.pending_changes(), 0);
}

#[test]
fn a_recording_without_audio_upload_sends_only_its_begin_and_finish() {
    let server = FakeServer::new();
    server.set_upload_audio(false);
    let mut a = Device::new("sermon-free");
    a.start(&server);
    let (_, m) = record(&mut a, 5 * 60 + 20);
    a.settle(&server);

    assert_eq!(server.joined_files(&m.id, "audioPart").0, 0);
    assert_eq!(server.joined_files(&m.id, "uploadPart").0, 0);
    let finished = server.finished_sermons();
    assert_eq!(finished.len(), 1);
    assert!(finished[0].get("audioFileUid").is_none(), "{}", finished[0]);
    assert_eq!(finished[0]["uid"], m.id.as_str());
}

#[test]
fn a_sermons_transcript_arrives_beside_its_recording() {
    let server = FakeServer::new();
    let mut a = Device::new("sermon-text");
    a.start(&server);
    let (_, m) = record(&mut a, 3);
    a.settle(&server);
    assert!(linger_core::transcribe::load(&a.dir, &m.id).is_none());

    let text = br#"{"durationMs":3000,"segments":[{"startMs":0,"endMs":3000,"speaker":"A","text":"Turn to Romans 8."}]}"#;
    server.transcribed(&m.id, text);
    a.settle(&server);
    let t = linger_core::transcribe::load(&a.dir, &m.id).expect("the transcript, beside the recording");
    assert_eq!(t.text.segments[0].text, "Turn to Romans 8.");
    assert_eq!(linger_core::transcribe::all(&a.dir).len(), 1);
}
