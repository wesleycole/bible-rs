//! Two devices on one real Convex: a note written in one `StudyStore` appears in the other, then an
//! edit and a stale edit made offline become the note and its conflict copy on both. Runs against
//! the local backend `server/scripts/library-local.sh` starts (it prints `LINGER_TEST_CONVEX_URL`
//! and `LINGER_TEST_TOKEN`); without them it says so and passes, so `cargo test` needs no server.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use linger_core::auth::StaticToken;
use linger_core::books::{BookId, VerseRef};
use linger_core::study::StudyStore;
use linger_core::sync::{self, ConvexBackend, SyncHandle, SyncStatus};

struct Device {
    dir: PathBuf,
    store: StudyStore,
    sync: Option<SyncHandle>,
}

impl Device {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("linger-local-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = StudyStore::open(&dir.join("study.sqlite")).unwrap();
        Self { dir, store, sync: None }
    }

    fn start(&mut self, url: &str, token: &str) {
        let backend = ConvexBackend::new(url).unwrap();
        let handle = sync::start(&self.dir, Box::new(backend), Arc::new(StaticToken(token.into()))).unwrap();
        self.store.on_outbox(handle.notifier());
        self.sync = Some(handle);
    }

    fn stop(&mut self) {
        self.sync = None;
        self.store.on_outbox(|| {});
    }

    /// Everything here sent, and the feed read at least to `cursor`.
    fn settle(&mut self, cursor: u64) -> u64 {
        let sync = self.sync.as_ref().unwrap();
        eventually(|| self.store.pending_changes() == 0);
        eventually(|| sync.status() == SyncStatus::Synced && sync.cursor() >= cursor);
        self.store.refresh().unwrap();
        sync.cursor()
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.sync = None;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn eventually(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_note_written_on_one_device_appears_on_the_other_through_convex() {
    let (Ok(url), Ok(token)) = (std::env::var("LINGER_TEST_CONVEX_URL"), std::env::var("LINGER_TEST_TOKEN"))
    else {
        eprintln!(
            "skipped: run server/scripts/library-local.sh for LINGER_TEST_CONVEX_URL and LINGER_TEST_TOKEN"
        );
        return;
    };
    // A verse of its own per run, so a reused backend and token still start from nothing here.
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let v = VerseRef::new(BookId(18), (stamp % 40 + 1) as u8, 1);
    let (mut a, mut b) = (Device::new("a"), Device::new("b"));
    a.start(&url, &token);
    b.start(&url, &token);

    a.store.set_note(v, "Shall a man be more pure than his maker?").unwrap();
    let cursor = a.settle(0);
    let started = Instant::now();
    b.settle(cursor);
    assert_eq!(b.store.note(v).map(|n| n.body.as_str()), Some("Shall a man be more pure than his maker?"));
    assert!(started.elapsed() < Duration::from_secs(1), "the note took {:?} to arrive", started.elapsed());

    // B edits while offline against what it last saw; A's edit lands first.
    b.stop();
    b.store.set_note(v, "B, offline").unwrap();
    a.store.set_note(v, "A, online").unwrap();
    let cursor = a.settle(cursor + 1);
    b.start(&url, &token);
    let cursor = b.settle(cursor);
    a.settle(cursor);
    for d in [&a, &b] {
        assert_eq!(d.store.note(v).map(|n| n.body.as_str()), Some("A, online"), "the later edit stands");
        let copies: Vec<_> = d.store.conflict_copies(v).iter().map(|n| n.body.clone()).collect();
        assert_eq!(copies, ["B, offline"], "the stale edit is kept beside it");
    }
}
