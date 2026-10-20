//! Local study data: highlights, notes, bookmarks, settings and Ask conversations, in one SQLite
//! file.
//!
//! Everything is keyed by canonical verse reference (not by translation), so a highlight made
//! while reading the KJV shows up in the WEB too. The tables are mirrored in memory — they hold
//! hundreds of rows, not millions — so the reader can look them up per verse every frame; writes go
//! straight through to SQLite (WAL mode, a few hundred microseconds each).
//!
//! Highlights and bookmarks are also the library cache (docs/plans/library.md, "The sync engine"):
//! keyed by [`VerseId`], with the `seq` of the server's copy and a `deleted` flag for tombstones.
//! Every local change to them goes into the `outbox` in the same transaction; the sync engine
//! (`sync.rs`, on its own connection) sends it and applies the change feed here, and
//! [`StudyStore::refresh`] picks up what it wrote.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::books::{BookId, ChapterIdx, VerseRef};
#[cfg(feature = "sync")]
use crate::records::FeedRecord;
use crate::records::{Anchor, AnchorOrigin, Item, ItemKind, ItemStatus, VerseId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HighlightColor {
    Yellow,
    Green,
    Blue,
    Pink,
    Purple,
    Orange,
}

impl HighlightColor {
    /// In key order: 1–6 apply these, 0 clears. The stored value is the index, so new colours
    /// only ever go at the end.
    pub const ALL: [HighlightColor; 6] = [
        HighlightColor::Yellow,
        HighlightColor::Green,
        HighlightColor::Blue,
        HighlightColor::Pink,
        HighlightColor::Purple,
        HighlightColor::Orange,
    ];

    pub fn name(self) -> &'static str {
        match self {
            HighlightColor::Yellow => "yellow",
            HighlightColor::Green => "green",
            HighlightColor::Blue => "blue",
            HighlightColor::Pink => "pink",
            HighlightColor::Purple => "purple",
            HighlightColor::Orange => "orange",
        }
    }

    fn from_i64(v: i64) -> Option<Self> {
        Self::ALL.get(usize::try_from(v).ok()?).copied()
    }

    fn to_i64(self) -> i64 {
        Self::ALL.iter().position(|c| *c == self).unwrap() as i64
    }
}

/// A verse's own note: the note item whose only user anchor is that verse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// The note item's uid.
    pub uid: String,
    pub body: String,
    pub created: i64,
    pub updated: i64,
    /// The last change to it this session came from another device (through the change feed),
    /// not from this one: the notes page says so when it's open.
    pub edited_elsewhere: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub label: String,
    pub created: i64,
}

pub struct StudyStore {
    conn: Connection,
    highlights: HashMap<u32, (HighlightColor, i64)>,
    notes: HashMap<u32, Note>,
    /// Conflict copies of each verse's own note, oldest first.
    copies: HashMap<u32, Vec<Note>>,
    bookmarks: BTreeMap<u32, Bookmark>,
    generation: u64,
    /// SQLite's `data_version` when the mirrors were last read: it moves when another connection
    /// (the sync engine's) commits.
    data_version: i64,
    /// Told after a change is queued in the outbox, so the sync engine sends it now.
    on_outbox: Option<Box<dyn Fn() + Send>>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS highlights(
    book TEXT NOT NULL, chapter INTEGER NOT NULL, verse INTEGER NOT NULL,
    color INTEGER NOT NULL, created INTEGER NOT NULL,
    PRIMARY KEY(book, chapter, verse));
CREATE TABLE IF NOT EXISTS notes(
    book TEXT NOT NULL, chapter INTEGER NOT NULL, verse INTEGER NOT NULL,
    body TEXT NOT NULL, created INTEGER NOT NULL, updated INTEGER NOT NULL,
    PRIMARY KEY(book, chapter, verse));
CREATE TABLE IF NOT EXISTS bookmarks(
    book TEXT NOT NULL, chapter INTEGER NOT NULL, verse INTEGER NOT NULL,
    label TEXT NOT NULL, created INTEGER NOT NULL,
    PRIMARY KEY(book, chapter, verse));
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS reading(
    started INTEGER PRIMARY KEY, book TEXT NOT NULL, chapter INTEGER NOT NULL,
    translation TEXT NOT NULL, seconds INTEGER NOT NULL, furthest INTEGER NOT NULL,
    verses INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS plans(
    id TEXT PRIMARY KEY, started INTEGER NOT NULL, finished INTEGER);
CREATE TABLE IF NOT EXISTS plan_reads(
    plan TEXT NOT NULL, day INTEGER NOT NULL, passage INTEGER NOT NULL, at INTEGER NOT NULL,
    PRIMARY KEY(plan, day, passage));
CREATE TABLE IF NOT EXISTS chats(
    id INTEGER PRIMARY KEY, title TEXT NOT NULL, created INTEGER NOT NULL,
    updated INTEGER NOT NULL, messages TEXT NOT NULL, api TEXT NOT NULL, seq INTEGER NOT NULL);
";

/// The `PRAGMA user_version` this build migrates a file to.
const VERSION: i64 = 2;

/// Migration 1 (M1): highlights and bookmarks become the library cache, keyed by [`VerseId`],
/// with the outbox, the feed cursor and recorded conflicts beside them.
const MIGRATION_1: &str = "
CREATE TABLE highlights_v1(
    verse INTEGER PRIMARY KEY, color INTEGER NOT NULL, created INTEGER NOT NULL,
    updated INTEGER NOT NULL, seq INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0);
CREATE TABLE bookmarks_v1(
    verse INTEGER PRIMARY KEY, label TEXT NOT NULL, created INTEGER NOT NULL,
    updated INTEGER NOT NULL, seq INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0);
CREATE TABLE outbox(
    id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, key INTEGER NOT NULL,
    payload TEXT NOT NULL, base_seq INTEGER NOT NULL, rev INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT, next_at INTEGER NOT NULL DEFAULT 0,
    created INTEGER NOT NULL, UNIQUE(kind, key));
CREATE TABLE sync_state(key TEXT PRIMARY KEY, value INTEGER NOT NULL);
CREATE TABLE conflicts(
    id INTEGER PRIMARY KEY, kind TEXT NOT NULL, key INTEGER NOT NULL, mine TEXT NOT NULL,
    theirs TEXT NOT NULL, at INTEGER NOT NULL);
";

/// Migration 2 (M1): notes become `note` items, the library's own record (several to a verse,
/// anchored to verses), with a per-chapter index of their anchors; chats get a `uid` and the
/// server's `seq` (their local order moves to `ord`); `files` keeps what the cache has downloaded.
/// The outbox's `key` loses its type, so it holds a verse id for a highlight or bookmark and a
/// uid for a note or chat.
const MIGRATION_2: &str = "
CREATE TABLE items(
    uid TEXT PRIMARY KEY, kind TEXT NOT NULL, seq INTEGER NOT NULL DEFAULT 0,
    deleted INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL, updated INTEGER NOT NULL,
    record TEXT NOT NULL);
CREATE TABLE item_anchors(
    uid TEXT NOT NULL, book INTEGER NOT NULL, chapter INTEGER NOT NULL,
    first INTEGER NOT NULL, last INTEGER NOT NULL, origin TEXT NOT NULL);
CREATE INDEX item_anchors_chapter ON item_anchors(book, chapter);
CREATE INDEX item_anchors_uid ON item_anchors(uid);
CREATE TABLE files(
    uid TEXT PRIMARY KEY, owner TEXT, role TEXT NOT NULL, url TEXT NOT NULL,
    bytes INTEGER NOT NULL, path TEXT NOT NULL);
ALTER TABLE chats RENAME COLUMN seq TO ord;
ALTER TABLE chats ADD COLUMN uid TEXT;
ALTER TABLE chats ADD COLUMN seq INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chats ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE chats ADD COLUMN file_uid TEXT;
ALTER TABLE chats ADD COLUMN url TEXT;
ALTER TABLE chats ADD COLUMN fetched INTEGER NOT NULL DEFAULT 1;
CREATE TABLE outbox_v2(
    id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, key NOT NULL,
    payload TEXT NOT NULL, base_seq INTEGER NOT NULL, rev INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT, next_at INTEGER NOT NULL DEFAULT 0,
    created INTEGER NOT NULL, UNIQUE(kind, key));
INSERT INTO outbox_v2 SELECT * FROM outbox;
DROP TABLE outbox;
ALTER TABLE outbox_v2 RENAME TO outbox;
";

/// Brings a file (new, or from any earlier version) up to [`VERSION`], one transaction a step.
fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > VERSION {
        bail!("study.sqlite was written by a newer Linger (schema {version})");
    }
    if version < 1 {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(SCHEMA)?;
        tx.execute_batch(MIGRATION_1)?;
        // The packed (book, chapter, verse) keys become VerseIds, and everything already here
        // goes to the server once, through the outbox, the first time this device is signed in.
        let t = now();
        let mut queue = Vec::new();
        let mut rows = Vec::new();
        {
            let mut st = tx.prepare("SELECT book, chapter, verse, color, created FROM highlights")?;
            let mut q = st.query([])?;
            while let Some(r) = q.next()? {
                rows.push((
                    r.get::<_, String>(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ));
            }
        }
        for (b, c, v, color, created) in rows {
            let (Some(vr), Some(color)) = (verse_from_row(&b, c, v), HighlightColor::from_i64(color)) else {
                continue;
            };
            let id = VerseId::from(vr);
            tx.execute(
                "INSERT OR REPLACE INTO highlights_v1(verse, color, created, updated) VALUES(?1, ?2, ?3, ?3)",
                params![id.0, color.to_i64(), created],
            )?;
            queue.push((Kind::Highlight, id, json!(color.name())));
        }
        let mut rows = Vec::new();
        {
            let mut st = tx.prepare("SELECT book, chapter, verse, label, created FROM bookmarks")?;
            let mut q = st.query([])?;
            while let Some(r) = q.next()? {
                rows.push((
                    r.get::<_, String>(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                ));
            }
        }
        for (b, c, v, label, created) in rows {
            let Some(vr) = verse_from_row(&b, c, v) else { continue };
            let id = VerseId::from(vr);
            tx.execute(
                "INSERT OR REPLACE INTO bookmarks_v1(verse, label, created, updated) VALUES(?1, ?2, ?3, ?3)",
                params![id.0, label, created],
            )?;
            queue.push((Kind::Bookmark, id, json!(label)));
        }
        tx.execute_batch(
            "DROP TABLE highlights; ALTER TABLE highlights_v1 RENAME TO highlights;
             DROP TABLE bookmarks; ALTER TABLE bookmarks_v1 RENAME TO bookmarks;",
        )?;
        for (kind, id, value) in queue {
            enqueue(&tx, kind, id, value, t)?;
        }
        tx.pragma_update(None, "user_version", 1)?;
        tx.commit()?;
    }
    if version < 2 {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(MIGRATION_2)?;
        let t = now();
        // Each note becomes the verse's own note item and goes to the server once, as do chats.
        let mut rows = Vec::new();
        {
            let mut st = tx.prepare("SELECT book, chapter, verse, body, created, updated FROM notes")?;
            let mut q = st.query([])?;
            while let Some(r) = q.next()? {
                rows.push((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ));
            }
        }
        for (b, c, v, body, created, updated) in rows {
            let Some(vr) = verse_from_row(&b, c, v) else { continue };
            let uid = new_uid();
            put_item(&tx, &note_item(&uid, VerseId::from(vr), &body, created), created, updated)?;
            if VerseId::from(vr).is_valid() {
                enqueue_uid(&tx, Kind::Note, &uid, t)?;
            }
        }
        tx.execute("DROP TABLE notes", [])?;
        let ids: Vec<i64> =
            tx.prepare("SELECT id FROM chats")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        for id in ids {
            let uid = new_uid();
            tx.execute("UPDATE chats SET uid = ?2 WHERE id = ?1", params![id, uid])?;
            enqueue_uid(&tx, Kind::Chat, &uid, t)?;
        }
        tx.execute_batch("CREATE UNIQUE INDEX chats_uid ON chats(uid);")?;
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
    }
    Ok(())
}

/// A new record id: a UUID, as every device makes them.
fn new_uid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A note as its verse's own note item: one user anchor on `verse`. `created` is Unix seconds.
fn note_item(uid: &str, verse: VerseId, text: &str, created: i64) -> Item {
    Item {
        uid: uid.into(),
        seq: 0,
        deleted_at: None,
        kind: ItemKind::Note,
        title: String::new(),
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
        conflict_of: None,
        captured_at: created.max(0) as u64 * 1000,
        updated_at: None,
        unreviewed: false,
        anchors: vec![Anchor { from: verse, to: verse, origin: AnchorOrigin::User, at_ms: None, page: None }]
            .into_iter()
            .chain(typed_anchors(text))
            .collect(),
        files: Vec::new(),
    }
}

/// Sets a note's text, with its `typed` anchors: every reference written in it (`find_all`), on
/// the canonical grid, without repeats. Its other anchors stay.
fn set_note_text(item: &mut Item, text: &str) {
    item.text = Some(text.into());
    item.anchors.retain(|a| a.origin != AnchorOrigin::Typed);
    item.anchors.extend(typed_anchors(text));
}

fn typed_anchors(text: &str) -> Vec<Anchor> {
    let mut out: Vec<Anchor> = Vec::new();
    for (_, r) in crate::reference::find_all(text) {
        let (first, last) = match r.verse {
            Some(v) => (v, r.verse_end.filter(|&e| e > v).unwrap_or(v)),
            None => (1, r.chapter_idx().kjv_verses()),
        };
        let (from, to) = (VerseId::new(r.book, r.chapter, first), VerseId::new(r.book, r.chapter, last));
        if from.is_valid() && to.is_valid() && !out.iter().any(|a| (a.from, a.to) == (from, to)) {
            out.push(Anchor { from, to, origin: AnchorOrigin::Typed, at_ms: None, page: None });
        }
    }
    out
}

/// Writes an item into the cache, with its anchors in the per-chapter index (none once deleted).
/// `created` and `updated` are Unix seconds on this device. The `seq` column only moves up: it is
/// the server's, which a local edit (made from a record whose JSON `seq` may be older) never
/// changes.
fn put_item(tx: &Connection, item: &Item, created: i64, updated: i64) -> Result<()> {
    let kind = serde_json::to_value(item.kind)?;
    tx.execute(
        "INSERT INTO items(uid, kind, seq, deleted, created, updated, record) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(uid) DO UPDATE SET kind = excluded.kind, seq = MAX(seq, excluded.seq), deleted = excluded.deleted,
           created = excluded.created, updated = excluded.updated, record = excluded.record",
        params![
            item.uid,
            kind.as_str().unwrap_or("note"),
            item.seq as i64,
            item.deleted_at.is_some() as i64,
            created,
            updated,
            serde_json::to_string(item)?
        ],
    )?;
    tx.execute("DELETE FROM item_anchors WHERE uid = ?1", [&item.uid])?;
    if item.deleted_at.is_none() {
        for a in &item.anchors {
            for (book, chapter, first, last) in chapter_spans(a.from, a.to) {
                tx.execute(
                    "INSERT INTO item_anchors(uid, book, chapter, first, last, origin) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    params![item.uid, book, chapter, first, last, serde_json::to_value(a.origin)?.as_str()],
                )?;
            }
        }
    }
    // Its text files, for the engine to fetch (they're named by their bytes, so never rewritten).
    let text: Vec<&crate::records::FileRef> = if item.deleted_at.is_none() {
        item.files.iter().filter(|f| f.role == crate::records::FileRole::Text).collect()
    } else {
        Vec::new()
    };
    let known: Vec<String> = tx
        .prepare("SELECT uid FROM files WHERE owner = ?1")?
        .query_map([&item.uid], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for uid in known.iter().filter(|k| !text.iter().any(|f| &f.uid == *k)) {
        tx.execute("DELETE FROM files WHERE uid = ?1", [uid])?;
    }
    for f in text {
        tx.execute(
            "INSERT OR IGNORE INTO files(uid, owner, role, url, bytes, path) VALUES(?1, ?2, 'text', ?3, ?4, '')",
            params![f.uid, item.uid, f.url, f.bytes as i64],
        )?;
    }
    Ok(())
}

/// The item `uid` as the cache holds it. Its `seq` is the column's, the server's: the record JSON
/// of a note made here keeps whatever it was written with.
fn load_item(conn: &Connection, uid: &str) -> Result<Option<Item>> {
    let row: Option<(String, i64)> = conn
        .query_row("SELECT record, seq FROM items WHERE uid = ?1", [uid], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((record, seq)) = row else { return Ok(None) };
    let mut item: Item = serde_json::from_str(&record)?;
    item.seq = seq as u64;
    Ok(Some(item))
}

/// Deletes an item here (a tombstone, so a later write is made against its `seq`).
fn delete_item(tx: &Connection, uid: &str, t: i64) -> Result<bool> {
    let Some(mut item) = load_item(tx, uid)? else { return Ok(false) };
    if item.deleted_at.is_some() {
        return Ok(false);
    }
    item.deleted_at = Some(t.max(0) as u64 * 1000);
    tx.execute(
        "UPDATE items SET deleted = 1, updated = ?2, record = ?3 WHERE uid = ?1",
        params![uid, t, serde_json::to_string(&item)?],
    )?;
    tx.execute("DELETE FROM item_anchors WHERE uid = ?1", [uid])?;
    tx.execute("DELETE FROM files WHERE owner = ?1", [uid])?;
    Ok(true)
}

/// The verse whose own note `item` could be: a live note whose only user anchor is that one
/// verse, and not a conflict copy of a note still live (`live` holds the live notes' uids). A
/// copy whose original is gone is an ordinary note on its verse.
fn own_verse(item: &Item, live: &std::collections::HashSet<String>) -> Option<VerseRef> {
    if item.kind != ItemKind::Note
        || item.deleted_at.is_some()
        || item.conflict_of.as_ref().is_some_and(|of| live.contains(of))
    {
        return None;
    }
    let mut user = item.anchors.iter().filter(|a| a.origin == AnchorOrigin::User);
    let a = user.next()?;
    if user.next().is_some() || a.from != a.to {
        return None;
    }
    verse_of(a.from.0 as i64)
}

/// Every verse's own note in the cache, and the live conflict copies of each. A verse with several
/// notes (two devices each wrote one before either saw the other's) has the oldest as its own; the
/// rest stay in the library.
type Notes = (HashMap<u32, Note>, HashMap<u32, Vec<Note>>);

fn own_notes(conn: &Connection) -> Result<Notes> {
    let mut st = conn.prepare(
        "SELECT uid, created, updated, record FROM items WHERE kind = 'note' AND deleted = 0 ORDER BY created, uid",
    )?;
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?))
    })?;
    // A record this build can't parse is skipped; a failing read is an error.
    let mut notes: Vec<(String, i64, i64, Item)> = Vec::new();
    for row in rows {
        let (uid, c, u, record) = row?;
        if let Ok(item) = serde_json::from_str::<Item>(&record) {
            notes.push((uid, c, u, item));
        }
    }
    let live = notes.iter().map(|n| n.0.clone()).collect();
    let note = |uid: &str, created, updated, item: &Item| Note {
        uid: uid.into(),
        body: item.text.clone().unwrap_or_default(),
        created,
        updated,
        edited_elsewhere: false,
    };
    let mut own: HashMap<u32, Note> = HashMap::new();
    for (uid, created, updated, item) in &notes {
        if let Some(v) = own_verse(item, &live) {
            own.entry(v.key()).or_insert_with(|| note(uid, *created, *updated, item));
        }
    }
    let verse_of_own: HashMap<&str, u32> = own.iter().map(|(v, n)| (n.uid.as_str(), *v)).collect();
    let mut copies: HashMap<u32, Vec<Note>> = HashMap::new();
    for (uid, created, updated, item) in &notes {
        if let Some(v) = item.conflict_of.as_deref().and_then(|of| verse_of_own.get(of)) {
            copies.entry(*v).or_default().push(note(uid, *created, *updated, item));
        }
    }
    Ok((own, copies))
}

/// Saves `body` as the verse's own note `own` (a new note item when there is none), or deletes
/// it for a blank body, and queues the change. Returns the note that now stands, if any.
fn write_note(
    tx: &Connection,
    own: Option<&Note>,
    v: VerseRef,
    body: &str,
    created: i64,
    updated: i64,
) -> Result<Option<Note>> {
    // A verse off the canonical grid keeps its note on this device (the server would refuse it).
    let queue = VerseId::from(v).is_valid();
    if body.trim().is_empty() {
        if let Some(own) = own
            && delete_item(tx, &own.uid, updated)?
            && queue
        {
            enqueue_uid(tx, Kind::Note, &own.uid, updated)?;
        }
        return Ok(None);
    }
    if let Some(own) = own.filter(|n| n.body == body) {
        // Nothing changed: no write, and the edit time stays the server's.
        return Ok(Some(own.clone()));
    }
    let (uid, created) = own.map_or_else(|| (new_uid(), created), |n| (n.uid.clone(), n.created));
    let mut item = load_item(tx, &uid)?.unwrap_or_else(|| note_item(&uid, VerseId::from(v), body, created));
    set_note_text(&mut item, body);
    put_item(tx, &item, created, updated)?;
    if queue {
        enqueue_uid(tx, Kind::Note, &uid, updated)?;
    }
    Ok(Some(Note { uid, body: body.into(), created, updated, edited_elsewhere: false }))
}

/// A verse range split at chapter breaks, as (book, chapter, first, last) on the KJV grid.
fn chapter_spans(from: VerseId, to: VerseId) -> Vec<(u32, u32, u32, u32)> {
    let (Some(a), Some(b)) = (from.verse_ref(), to.verse_ref()) else { return Vec::new() };
    let mut out = Vec::new();
    let mut idx = ChapterIdx::new(a.book, a.chapter);
    while let Some(c) = idx {
        let first = if (c.book(), c.chapter()) == (a.book, a.chapter) { a.verse } else { 1 };
        let last = if (c.book(), c.chapter()) == (b.book, b.chapter) { b.verse } else { c.kjv_verses() };
        out.push((
            c.book().index() as u32,
            c.chapter() as u32,
            VerseId::new(c.book(), c.chapter(), first).0,
            VerseId::new(c.book(), c.chapter(), last).0,
        ));
        if (c.book(), c.chapter()) == (b.book, b.chapter) || out.len() > 1_189 {
            break;
        }
        idx = c.next();
    }
    out
}

/// A kind of synced record, as the outbox and the cache name it: highlights and bookmarks are
/// kept one per verse, items and chats by uid. `Note` is every item, not only notes: the outbox
/// sends a sermon's or page's change as `items.updateDetails` (its stored name stays "note").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Highlight,
    Bookmark,
    Note,
    Chat,
}

impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Highlight => "highlight",
            Kind::Bookmark => "bookmark",
            Kind::Note => "note",
            Kind::Chat => "chat",
        }
    }

    #[cfg(feature = "sync")]
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        [Kind::Highlight, Kind::Bookmark, Kind::Note, Kind::Chat].into_iter().find(|k| k.name() == name)
    }

    fn table(self) -> &'static str {
        match self {
            Kind::Highlight => "highlights",
            Kind::Bookmark => "bookmarks",
            Kind::Note => "items",
            Kind::Chat => "chats",
        }
    }

    /// For a per-verse kind, the mutation that sets one, and the name of its value in the args and
    /// in a conflict answer.
    pub(crate) fn mutation(self) -> (&'static str, &'static str) {
        match self {
            Kind::Highlight => ("library/highlights:set", "color"),
            Kind::Bookmark => ("library/bookmarks:set", "label"),
            Kind::Note | Kind::Chat => unreachable!("{self:?} is kept by uid"),
        }
    }
}

/// Queues the note or chat `uid` in the outbox, against the `seq` of the copy in the cache. Its
/// arguments are read from the cache when it is sent, so a later edit only bumps the entry,
/// which keeps its place and its `base_seq`.
fn enqueue_uid(tx: &Connection, kind: Kind, uid: &str, t: i64) -> Result<()> {
    let base: i64 = tx
        .query_row(&format!("SELECT seq FROM {} WHERE uid = ?1", kind.table()), [uid], |r| r.get(0))
        .optional()?
        .unwrap_or(0);
    tx.execute(
        "INSERT INTO outbox(kind, key, payload, base_seq, created) VALUES(?1, ?2, '{}', ?3, ?4)
         ON CONFLICT(kind, key) DO UPDATE SET rev = rev + 1, attempts = 0, last_error = NULL, next_at = 0",
        params![kind.name(), uid, base, t],
    )?;
    Ok(())
}

/// Queues `value` (a colour name or label, `null` to clear) for `verse` in the outbox, against the
/// `seq` of the copy in the cache. A change already waiting for the verse is replaced and keeps its
/// place and its `base_seq`: both edits began from that copy. Verses off the canonical grid stay
/// on this device (the server would refuse them).
fn enqueue(
    tx: &rusqlite::Transaction,
    kind: Kind,
    verse: VerseId,
    value: serde_json::Value,
    t: i64,
) -> Result<()> {
    if !verse.is_valid() {
        return Ok(());
    }
    let (base, created): (i64, i64) = tx
        .query_row(&format!("SELECT seq, created FROM {} WHERE verse = ?1", kind.table()), [verse.0], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
        .unwrap_or((0, t));
    let mut payload = json!({ "verse": verse.0, kind.mutation().1: value });
    if !payload[kind.mutation().1].is_null() {
        // In this device's milliseconds; the engine moves it to server time when it sends it.
        payload["createdAt"] = json!(created * 1000);
    }
    let payload = payload.to_string();
    tx.execute(
        "INSERT INTO outbox(kind, key, payload, base_seq, created) VALUES(?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(kind, key) DO UPDATE SET payload = excluded.payload, rev = rev + 1, attempts = 0,
           last_error = NULL, next_at = 0",
        params![kind.name(), verse.0, payload, base, t],
    )?;
    Ok(())
}

/// Sets (or, for `None`, clears) a highlight in the cache and queues it. Returns whether anything
/// changed.
fn put_highlight(
    tx: &rusqlite::Transaction,
    v: VerseRef,
    color: Option<HighlightColor>,
    created: i64,
) -> Result<bool> {
    let id = VerseId::from(v);
    let t = now();
    let changed = match color {
        Some(c) => tx.execute(
            "INSERT INTO highlights(verse, color, created, updated) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(verse) DO UPDATE SET color = excluded.color, updated = excluded.updated,
               created = CASE WHEN deleted = 1 THEN excluded.created ELSE created END, deleted = 0",
            params![id.0, c.to_i64(), created, t],
        )?,
        None => tx.execute(
            "UPDATE highlights SET deleted = 1, updated = ?2 WHERE verse = ?1 AND deleted = 0",
            params![id.0, t],
        )?,
    } > 0;
    if changed {
        enqueue(tx, Kind::Highlight, id, json!(color.map(HighlightColor::name)), t)?;
    }
    Ok(changed)
}

/// Sets (or, for `None`, removes) a bookmark in the cache and queues it.
fn put_bookmark(tx: &rusqlite::Transaction, v: VerseRef, label: Option<&str>, created: i64) -> Result<bool> {
    let id = VerseId::from(v);
    let t = now();
    let changed = match label {
        Some(label) => tx.execute(
            "INSERT INTO bookmarks(verse, label, created, updated) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(verse) DO UPDATE SET label = excluded.label, updated = excluded.updated,
               created = CASE WHEN deleted = 1 THEN excluded.created ELSE created END, deleted = 0",
            params![id.0, label, created, t],
        )?,
        None => tx.execute(
            "UPDATE bookmarks SET deleted = 1, updated = ?2 WHERE verse = ?1 AND deleted = 0",
            params![id.0, t],
        )?,
    } > 0;
    if changed {
        enqueue(tx, Kind::Bookmark, id, json!(label), t)?;
    }
    Ok(changed)
}

/// An edit the server didn't take because another device had changed the record first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConflict {
    /// `highlight` or `bookmark`.
    pub kind: String,
    pub verse: VerseRef,
    /// This device's edit and the server's copy that stood, as JSON values (`null` for cleared).
    pub mine: String,
    pub theirs: String,
    /// Unix seconds.
    pub at: i64,
}

/// An Ask conversation as Recents lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatSummary {
    pub id: i64,
    pub title: String,
    /// Unix seconds of the last answer.
    pub updated: i64,
}

/// A saved Ask conversation: what the panel shows, and the API's own history (JSON, see
/// [`assistant::Conversation`](crate::assistant::Conversation)) to carry on from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedChat {
    pub id: i64,
    pub title: String,
    pub messages: Vec<crate::chat::Message>,
    pub api: String,
}

/// A stretch of reading one chapter: when it started, for how long, and how far it got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingSession {
    /// Unix seconds; also the session's identity.
    pub started: i64,
    pub book: BookId,
    pub chapter: u8,
    pub translation: String,
    /// Time actually spent reading (idle time is not counted).
    pub seconds: u32,
    /// The furthest verse reached, and how many verses the chapter has.
    pub furthest: u8,
    pub verses: u8,
}

impl ReadingSession {
    pub fn chapter_idx(&self) -> Option<ChapterIdx> {
        ChapterIdx::new(self.book, self.chapter)
    }

    /// Reached the chapter's last verse.
    pub fn finished(&self) -> bool {
        self.verses > 0 && self.furthest >= self.verses
    }

    /// How much of the chapter was read, 0–1.
    pub fn progress(&self) -> f32 {
        if self.verses == 0 { 0.0 } else { (self.furthest as f32 / self.verses as f32).min(1.0) }
    }
}

/// A reading plan under way (or finished): which, and when it began (the start of that day).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanProgress {
    pub id: String,
    pub started: i64,
    pub finished: Option<i64>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn verse_from_row(book: &str, chapter: i64, verse: i64) -> Option<VerseRef> {
    let book = BookId::from_code(book)?;
    Some(VerseRef::new(book, u8::try_from(chapter).ok()?, u8::try_from(verse).ok()?))
}

/// The verse a cache row's id names. Unlike [`VerseId::verse_ref`] this doesn't check the KJV's
/// verse counts: a verse only another versification has can still be highlighted on this device.
fn verse_of(id: i64) -> Option<VerseRef> {
    let id = u32::try_from(id).ok()?;
    let book = BookId::new((id / 1_000_000) as usize)?;
    Some(VerseRef::new(book, u8::try_from(id / 1_000 % 1_000).ok()?, u8::try_from(id % 1_000).ok()?))
}

impl StudyStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(connect(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        migrate(&mut conn)?;
        let mut s = Self {
            conn,
            highlights: HashMap::new(),
            notes: HashMap::new(),
            copies: HashMap::new(),
            bookmarks: BTreeMap::new(),
            generation: 1,
            data_version: 0,
            on_outbox: None,
        };
        s.data_version = s.conn.pragma_query_value(None, "data_version", |r| r.get(0))?;
        s.reload()?;
        Ok(s)
    }

    /// Reads again what the sync engine wrote since the last look (records from the change feed,
    /// conflict answers). Cheap enough to call every frame; bumps [`generation`](Self::generation)
    /// when something changed.
    pub fn refresh(&mut self) -> Result<bool> {
        let v: i64 = self.conn.pragma_query_value(None, "data_version", |r| r.get(0))?;
        if v == self.data_version {
            return Ok(false);
        }
        self.data_version = v;
        self.reload()?;
        Ok(true)
    }

    /// Calls `f` after each change queued for the server, so the sync engine sends it at once.
    pub fn on_outbox(&mut self, f: impl Fn() + Send + 'static) {
        self.on_outbox = Some(Box::new(f));
    }

    fn queued(&self) {
        if let Some(f) = &self.on_outbox {
            f();
        }
    }

    /// Changes made here that the server hasn't confirmed yet.
    pub fn pending_changes(&self) -> usize {
        pending(&self.conn).unwrap_or(0)
    }

    /// Edits the server refused because another device had changed the record since, newest
    /// first: the server's copy stood and is what this device now shows.
    /// The order is part of the contract: `linger sync` takes the head to report a run's own.
    pub fn conflicts(&self) -> Vec<SyncConflict> {
        let Ok(mut st) =
            self.conn.prepare_cached("SELECT kind, key, mine, theirs, at FROM conflicts ORDER BY id DESC")
        else {
            return Vec::new();
        };
        st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map(|rows| {
            rows.filter_map(|r| r.ok())
                .filter_map(|(kind, key, mine, theirs, at)| {
                    Some(SyncConflict { kind, verse: verse_of(key)?, mine, theirs, at })
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// Signing out: forgets the library cache (highlights, bookmarks), the outbox, the cursor and
    /// recorded conflicts, so nothing of this account stays. Wait for the outbox first.
    pub fn clear_library(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM highlights; DELETE FROM bookmarks; DELETE FROM items; DELETE FROM item_anchors;
             DELETE FROM files; DELETE FROM chats; DELETE FROM outbox; DELETE FROM sync_state;
             DELETE FROM conflicts;",
        )?;
        if let Some(dir) = self.conn.path().and_then(|p| Path::new(p).parent()) {
            let _ = std::fs::remove_dir_all(dir.join("files"));
        }
        self.reload()
    }

    fn reload(&mut self) -> Result<()> {
        self.highlights.clear();
        self.bookmarks.clear();
        let mut st = self.conn.prepare("SELECT verse, color, created FROM highlights WHERE deleted = 0")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get(2)?)))?;
        for row in rows {
            let (id, color, created) = row?;
            if let (Some(vr), Some(color)) = (verse_of(id), HighlightColor::from_i64(color)) {
                self.highlights.insert(vr.key(), (color, created));
            }
        }
        let (notes, copies) = own_notes(&self.conn)?;
        // A note whose text changed under this session came from another device.
        let before: HashMap<&str, &Note> = self.notes.values().map(|n| (n.uid.as_str(), n)).collect();
        let notes = notes
            .into_iter()
            .map(|(v, mut n)| {
                if let Some(old) = before.get(n.uid.as_str()) {
                    n.edited_elsewhere = old.body != n.body || old.edited_elsewhere;
                }
                (v, n)
            })
            .collect();
        self.notes = notes;
        self.copies = copies;
        let mut st = self.conn.prepare("SELECT verse, label, created FROM bookmarks WHERE deleted = 0")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get(2)?)))?;
        for row in rows {
            let (id, label, created) = row?;
            if let Some(vr) = verse_of(id) {
                self.bookmarks.insert(vr.key(), Bookmark { label, created });
            }
        }
        self.generation += 1;
        Ok(())
    }

    /// Increments whenever study data changes (the reader uses it to invalidate layouts).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn highlight(&self, v: VerseRef) -> Option<HighlightColor> {
        self.highlights.get(&v.key()).map(|(c, _)| *c)
    }

    pub fn note(&self, v: VerseRef) -> Option<&Note> {
        self.notes.get(&v.key())
    }

    pub fn bookmark(&self, v: VerseRef) -> Option<&Bookmark> {
        self.bookmarks.get(&v.key())
    }

    pub fn is_bookmarked(&self, v: VerseRef) -> bool {
        self.bookmarks.contains_key(&v.key())
    }

    /// Highlights a verse (or clears it), and queues the change for the server.
    pub fn set_highlight(&mut self, v: VerseRef, color: Option<HighlightColor>) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let changed = put_highlight(&tx, v, color, now())?;
        tx.commit()?;
        match color {
            Some(c) => {
                let created = self.highlights.get(&v.key()).map_or_else(now, |(_, t)| *t);
                self.highlights.insert(v.key(), (c, created));
            }
            None => {
                self.highlights.remove(&v.key());
            }
        }
        self.generation += 1;
        if changed {
            self.queued();
        }
        Ok(())
    }

    /// Saves the verse's own note (made on first save); an empty (whitespace-only) body deletes
    /// it. Queues the change for the server.
    pub fn set_note(&mut self, v: VerseRef, body: &str) -> Result<()> {
        let t = now();
        let own = self.notes.get(&v.key()).cloned();
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let standing = write_note(&tx, own.as_ref(), v, body, t, t)?;
        tx.commit()?;
        let changed = own.as_ref().map(|n| (&n.uid, &n.body)) != standing.as_ref().map(|n| (&n.uid, &n.body));
        match standing {
            Some(n) => self.notes.insert(v.key(), n),
            None => self.notes.remove(&v.key()),
        };
        self.generation += 1;
        if changed && VerseId::from(v).is_valid() {
            self.queued();
        }
        Ok(())
    }

    /// Bookmarks a verse with a label (or removes the bookmark), and queues the change.
    pub fn set_bookmark(&mut self, v: VerseRef, label: Option<&str>) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let changed = put_bookmark(&tx, v, label, now())?;
        tx.commit()?;
        match label {
            Some(label) => {
                let created = self.bookmarks.get(&v.key()).map_or_else(now, |b| b.created);
                self.bookmarks.insert(v.key(), Bookmark { label: label.to_string(), created });
            }
            None => {
                self.bookmarks.remove(&v.key());
            }
        }
        self.generation += 1;
        if changed {
            self.queued();
        }
        Ok(())
    }

    pub fn toggle_bookmark(&mut self, v: VerseRef) -> Result<bool> {
        let on = !self.is_bookmarked(v);
        self.set_bookmark(v, on.then_some(""))?;
        Ok(on)
    }

    pub fn highlights(&self) -> Vec<(VerseRef, HighlightColor)> {
        let mut v: Vec<_> =
            self.highlights.iter().filter_map(|(k, (c, _))| Some((VerseRef::from_key(*k)?, *c))).collect();
        v.sort_by_key(|(r, _)| *r);
        v
    }

    /// Highlights, the most recently made first.
    pub fn highlights_newest_first(&self) -> Vec<(VerseRef, HighlightColor)> {
        let mut v: Vec<_> = self
            .highlights
            .iter()
            .filter_map(|(k, (c, t))| Some((VerseRef::from_key(*k)?, *c, *t)))
            .collect();
        v.sort_by_key(|(r, _, t)| (std::cmp::Reverse(*t), *r));
        v.into_iter().map(|(r, c, _)| (r, c)).collect()
    }

    /// Highlights with when they were made, the most recent first.
    pub fn highlights_dated(&self) -> Vec<(VerseRef, HighlightColor, i64)> {
        let mut v: Vec<_> = self
            .highlights
            .iter()
            .filter_map(|(k, (c, t))| Some((VerseRef::from_key(*k)?, *c, *t)))
            .collect();
        v.sort_by_key(|(r, _, t)| (std::cmp::Reverse(*t), *r));
        v
    }

    /// The conflict copies of the verse's own note: edits another device made against a copy that
    /// had since changed, kept beside it rather than lost. Oldest first.
    pub fn conflict_copies(&self, v: VerseRef) -> &[Note] {
        self.copies.get(&v.key()).map_or(&[], Vec::as_slice)
    }

    /// Sets an item's details (a sermon's title, speaker and times; which sermon a slide or page
    /// was captured during, and where in it), and queues the change. Not for notes, whose title
    /// is their first line.
    pub fn set_item_details(
        &mut self,
        uid: &str,
        title: &str,
        details: Option<crate::records::ItemDetails>,
        sermon: Option<(String, Option<u64>)>,
    ) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let Some(mut item) =
            load_item(&tx, uid)?.filter(|i| i.kind != ItemKind::Note && i.deleted_at.is_none())
        else {
            anyhow::bail!("no item {uid} with details here");
        };
        item.title = title.into();
        item.details = details;
        (item.sermon_uid, item.offset_ms) = sermon.map_or((None, None), |(s, at)| (Some(s), at));
        let created: i64 = tx.query_row("SELECT created FROM items WHERE uid = ?1", [uid], |r| r.get(0))?;
        let t = now();
        put_item(&tx, &item, created, t)?;
        enqueue_uid(&tx, Kind::Note, uid, t)?;
        tx.commit()?;
        self.generation += 1;
        self.queued();
        Ok(())
    }

    /// Deletes a note by its uid (a conflict copy, say), and queues the change.
    pub fn delete_note(&mut self, uid: &str) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let t = now();
        let deleted = delete_item(&tx, uid, t)?;
        if deleted {
            enqueue_uid(&tx, Kind::Note, uid, t)?;
        }
        tx.commit()?;
        if deleted {
            self.reload()?;
            self.queued();
        }
        Ok(())
    }

    pub fn notes(&self) -> Vec<(VerseRef, &Note)> {
        let mut v: Vec<_> =
            self.notes.iter().filter_map(|(k, n)| Some((VerseRef::from_key(*k)?, n))).collect();
        v.sort_by_key(|(r, _)| *r);
        v
    }

    pub fn bookmarks(&self) -> Vec<(VerseRef, &Bookmark)> {
        self.bookmarks.iter().filter_map(|(k, b)| Some((VerseRef::from_key(*k)?, b))).collect()
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (self.highlights.len(), self.notes.len(), self.bookmarks.len())
    }

    /// Saves a reading session, replacing an earlier save of the same session.
    pub fn record_reading(&self, s: &ReadingSession) -> Result<()> {
        self.conn.execute(
            "INSERT INTO reading(started, book, chapter, translation, seconds, furthest, verses)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(started) DO UPDATE SET seconds = excluded.seconds, furthest = excluded.furthest,
               verses = excluded.verses, translation = excluded.translation",
            params![s.started, s.book.info().code, s.chapter, s.translation, s.seconds, s.furthest, s.verses],
        )?;
        Ok(())
    }

    /// Reading sessions that started at or after `since` (Unix seconds), newest first.
    pub fn reading_since(&self, since: i64) -> Vec<ReadingSession> {
        let Ok(mut st) = self.conn.prepare_cached(
            "SELECT started, book, chapter, translation, seconds, furthest, verses FROM reading
             WHERE started >= ?1 ORDER BY started DESC",
        ) else {
            return Vec::new();
        };
        let rows = st.query_map([since], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        });
        let Ok(rows) = rows else { return Vec::new() };
        rows.filter_map(|r| r.ok())
            .filter_map(|(started, book, chapter, translation, seconds, furthest, verses)| {
                Some(ReadingSession {
                    started,
                    book: BookId::from_code(&book)?,
                    chapter: u8::try_from(chapter).ok()?,
                    translation,
                    seconds: u32::try_from(seconds).ok()?,
                    furthest: u8::try_from(furthest).ok()?,
                    verses: u8::try_from(verses).ok()?,
                })
            })
            .collect()
    }

    /// Forgets reading history from `since` on (all of it for 0).
    pub fn clear_reading(&self, since: i64) -> Result<()> {
        self.conn.execute("DELETE FROM reading WHERE started >= ?1", [since])?;
        Ok(())
    }

    /// Saved conversations, most recently answered first.
    pub fn chats(&self) -> Vec<ChatSummary> {
        let Ok(mut st) = self
            .conn
            .prepare_cached("SELECT id, title, updated FROM chats WHERE deleted = 0 ORDER BY ord DESC")
        else {
            return Vec::new();
        };
        st.query_map([], |r| Ok(ChatSummary { id: r.get(0)?, title: r.get(1)?, updated: r.get(2)? }))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    pub fn chat(&self, id: i64) -> Option<SavedChat> {
        let (title, messages, api): (String, String, String) = self
            .conn
            // One synced from another device is listed at once and opens once its file is here.
            .query_row(
                "SELECT title, messages, api FROM chats WHERE id = ?1 AND deleted = 0 AND fetched = 1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .ok()??;
        Some(SavedChat { id, title, messages: serde_json::from_str(&messages).ok()?, api })
    }

    /// Saves a conversation, as a new one (`id` None) or over the one it continues, and queues it
    /// for the server. Returns its id, which is this device's own; the library knows it by `uid`.
    pub fn save_chat(
        &mut self,
        id: Option<i64>,
        title: &str,
        messages: &[crate::chat::Message],
        api: &str,
    ) -> Result<i64> {
        let messages = serde_json::to_string(messages)?;
        let at = now();
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // Order of saving, which the second-resolution times can't settle.
        let ord: i64 = tx.query_row("SELECT IFNULL(MAX(ord), 0) + 1 FROM chats", [], |r| r.get(0))?;
        let id = match id {
            // One deleted while its answer was coming is saved again, as a new one.
            Some(id)
                if tx.execute(
                    "UPDATE chats SET title = ?2, updated = ?3, messages = ?4, api = ?5, ord = ?6
                     WHERE id = ?1 AND deleted = 0",
                    params![id, title, at, messages, api, ord],
                )? > 0 =>
            {
                id
            }
            _ => {
                tx.execute(
                    "INSERT INTO chats(title, created, updated, messages, api, ord, uid)
                     VALUES(?1, ?2, ?2, ?3, ?4, ?5, ?6)",
                    params![title, at, messages, api, ord, new_uid()],
                )?;
                tx.last_insert_rowid()
            }
        };
        let uid: String = tx.query_row("SELECT uid FROM chats WHERE id = ?1", [id], |r| r.get(0))?;
        enqueue_uid(&tx, Kind::Chat, &uid, at)?;
        tx.commit()?;
        self.generation += 1;
        self.queued();
        Ok(id)
    }

    /// Deletes a conversation, from the library too.
    pub fn delete_chat(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let uid: Option<String> = tx
            .query_row(
                "UPDATE chats SET deleted = 1 WHERE id = ?1 AND deleted = 0 RETURNING uid",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(uid) = &uid {
            enqueue_uid(&tx, Kind::Chat, uid, now())?;
        }
        tx.commit()?;
        self.generation += 1;
        if uid.is_some() {
            self.queued();
        }
        Ok(())
    }

    /// Plans started, newest first.
    pub fn plans(&self) -> Vec<PlanProgress> {
        let Ok(mut st) =
            self.conn.prepare_cached("SELECT id, started, finished FROM plans ORDER BY started DESC")
        else {
            return Vec::new();
        };
        st.query_map([], |r| Ok(PlanProgress { id: r.get(0)?, started: r.get(1)?, finished: r.get(2)? }))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Starts (or restarts) a plan on the day containing `started`, forgetting earlier progress.
    pub fn start_plan(&mut self, id: &str, started: i64) -> Result<()> {
        self.conn.execute("DELETE FROM plan_reads WHERE plan = ?1", [id])?;
        self.conn.execute(
            "INSERT OR REPLACE INTO plans(id, started, finished) VALUES(?1, ?2, NULL)",
            params![id, started],
        )?;
        self.generation += 1;
        Ok(())
    }

    pub fn finish_plan(&mut self, id: &str, at: Option<i64>) -> Result<()> {
        self.conn.execute("UPDATE plans SET finished = ?2 WHERE id = ?1", params![id, at])?;
        self.generation += 1;
        Ok(())
    }

    pub fn stop_plan(&mut self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM plans WHERE id = ?1", [id])?;
        self.conn.execute("DELETE FROM plan_reads WHERE plan = ?1", [id])?;
        self.generation += 1;
        Ok(())
    }

    /// The passages ticked off in a plan, as (day, passage index) pairs.
    pub fn plan_reads(&self, id: &str) -> std::collections::HashSet<(u16, u8)> {
        let Ok(mut st) = self.conn.prepare_cached("SELECT day, passage FROM plan_reads WHERE plan = ?1")
        else {
            return Default::default();
        };
        st.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map(|rows| {
                rows.filter_map(|r| r.ok())
                    .filter_map(|(d, p)| Some((u16::try_from(d).ok()?, u8::try_from(p).ok()?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn set_plan_read(&mut self, id: &str, day: u16, passage: u8, read: bool) -> Result<()> {
        if read {
            self.conn.execute(
                "INSERT OR REPLACE INTO plan_reads(plan, day, passage, at) VALUES(?1, ?2, ?3, ?4)",
                params![id, day, passage, now()],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM plan_reads WHERE plan = ?1 AND day = ?2 AND passage = ?3",
                params![id, day, passage],
            )?;
        }
        self.generation += 1;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// JSON export of all study data. `text` supplies verse text for context (may return `None`).
    pub fn export_json(
        &self,
        translation: &str,
        text: impl Fn(VerseRef) -> Option<String>,
    ) -> Result<String> {
        self.export_json_with(translation, text, Include::STUDY)
    }

    /// The study data as JSON (re-importable), with what `include` asks for.
    pub fn export_json_with(
        &self,
        translation: &str,
        text: impl Fn(VerseRef) -> Option<String>,
        include: Include,
    ) -> Result<String> {
        let entry = |v: VerseRef| ExportVerse {
            reference: v.to_string(),
            book: v.book.info().code.to_string(),
            chapter: v.chapter,
            verse: v.verse,
            text: text(v),
        };
        let mut doc = ExportDoc {
            app: "linger".into(),
            format: 1,
            exported_at: rfc3339(now()),
            translation: translation.to_string(),
            history: Vec::new(),
            plans: Vec::new(),
            highlights: self
                .highlights()
                .into_iter()
                .map(|(v, c)| ExportHighlight {
                    verse: entry(v),
                    color: c,
                    created: rfc3339(self.highlights[&v.key()].1),
                })
                .collect(),
            notes: all_notes(&self.conn)?
                .into_iter()
                .map(|n| ExportNote {
                    verse: entry(n.verse),
                    note: n.body,
                    created: rfc3339(n.created),
                    updated: rfc3339(n.updated),
                    uid: Some(n.uid),
                    conflict_of: n.conflict_of,
                })
                .collect(),
            bookmarks: self
                .bookmarks()
                .into_iter()
                .map(|(v, b)| ExportBookmark {
                    verse: entry(v),
                    label: b.label.clone(),
                    created: rfc3339(b.created),
                })
                .collect(),
        };
        if !include.highlights {
            doc.highlights.clear();
        }
        if !include.notes {
            doc.notes.clear();
        }
        if !include.bookmarks {
            doc.bookmarks.clear();
        }
        if include.history {
            doc.history = self
                .reading_since(0)
                .into_iter()
                .map(|s| ExportReading {
                    started: rfc3339(s.started),
                    book: s.book.info().code.to_string(),
                    chapter: s.chapter,
                    translation: s.translation,
                    seconds: s.seconds,
                    furthest: s.furthest,
                    verses: s.verses,
                })
                .collect();
            doc.plans = self
                .plans()
                .into_iter()
                .map(|p| ExportPlan {
                    read: self.plan_reads(&p.id).into_iter().collect(),
                    id: p.id,
                    started: rfc3339(p.started),
                    finished: p.finished.map(rfc3339),
                })
                .collect();
        }
        Ok(serde_json::to_string_pretty(&doc)?)
    }

    pub fn export_markdown(&self, translation: &str, text: impl Fn(VerseRef) -> Option<String>) -> String {
        self.export_markdown_with(translation, text, Include::STUDY)
    }

    /// The study data as readable Markdown, with what `include` asks for.
    pub fn export_markdown_with(
        &self,
        translation: &str,
        text: impl Fn(VerseRef) -> Option<String>,
        include: Include,
    ) -> String {
        use std::fmt::Write;
        let mut md = String::new();
        let _ = writeln!(md, "# Bible study notes\n");
        let _ =
            writeln!(md, "Exported from linger on {} · verse text: {translation}\n", &rfc3339(now())[..10]);
        let bookmarks = if include.bookmarks { self.bookmarks() } else { Vec::new() };
        if !bookmarks.is_empty() {
            let _ = writeln!(md, "## Bookmarks\n");
            for (v, b) in bookmarks {
                if b.label.is_empty() {
                    let _ = writeln!(md, "- **{v}**");
                } else {
                    let _ = writeln!(md, "- **{v}** — {}", b.label);
                }
            }
            md.push('\n');
        }
        let highlights = if include.highlights { self.highlights() } else { Vec::new() };
        if !highlights.is_empty() {
            let _ = writeln!(md, "## Highlights\n");
            let mut book = None;
            for (v, c) in highlights {
                if book != Some(v.book) {
                    book = Some(v.book);
                    let _ = writeln!(md, "### {}\n", v.book.name());
                }
                let t = text(v).unwrap_or_default();
                let _ = writeln!(md, "- **{v}** ({}) {t}", c.name());
            }
            md.push('\n');
        }
        let notes = if include.notes { self.notes() } else { Vec::new() };
        if !notes.is_empty() {
            let _ = writeln!(md, "## Notes\n");
            for (v, n) in notes {
                let _ = writeln!(md, "### {v}\n");
                if let Some(t) = text(v) {
                    let _ = writeln!(md, "> {t}\n");
                }
                let _ = writeln!(md, "{}\n", n.body.trim_end());
            }
        }
        let history = if include.history { self.reading_since(0) } else { Vec::new() };
        if !history.is_empty() {
            let _ = writeln!(md, "## Reading history\n");
            for s in history {
                let minutes = (s.seconds + 30) / 60;
                let done = if s.finished() {
                    "finished".to_string()
                } else {
                    format!("{:.0} %", s.progress() * 100.0)
                };
                let _ = writeln!(
                    md,
                    "- {} — {} {} ({}), {minutes} min, {done}",
                    &rfc3339(s.started)[..10],
                    s.book.name(),
                    s.chapter,
                    s.translation
                );
            }
            md.push('\n');
        }
        md
    }

    /// Merges a JSON export back in (existing entries for the same verse are overwritten).
    pub fn import_json(&mut self, json: &str) -> Result<(usize, usize, usize)> {
        let doc: ExportDoc = serde_json::from_str(json).context("not a linger export")?;
        // Exports made before the rename say "bible-rs".
        if doc.app != "linger" && doc.app != "bible-rs" {
            bail!("not a linger export");
        }
        let verse = |e: &ExportVerse| -> Result<VerseRef> {
            let book = BookId::from_code(&e.book).with_context(|| format!("unknown book {}", e.book))?;
            Ok(VerseRef::new(book, e.chapter, e.verse))
        };
        let tx = rusqlite::Transaction::new_unchecked(&self.conn, rusqlite::TransactionBehavior::Immediate)?;
        let mut own = self.notes.clone();
        for h in &doc.highlights {
            put_highlight(
                &tx,
                verse(&h.verse)?,
                Some(h.color),
                parse_rfc3339(&h.created).unwrap_or_else(now),
            )?;
        }
        let mut seen = std::collections::HashSet::new();
        for n in &doc.notes {
            let v = verse(&n.verse)?;
            let legacy_first = seen.insert(v.key());
            if let Some(uid) = import_note(&tx, own.get(&v.key()), v, n, legacy_first)?
                && VerseId::from(v).is_valid()
            {
                enqueue_uid(&tx, Kind::Note, &uid, now())?;
            }
            own = own_notes(&tx)?.0;
        }
        for b in &doc.bookmarks {
            put_bookmark(
                &tx,
                verse(&b.verse)?,
                Some(&b.label),
                parse_rfc3339(&b.created).unwrap_or_else(now),
            )?;
        }
        for r in &doc.history {
            let Some(book) = BookId::from_code(&r.book) else { continue };
            tx.execute(
                "INSERT OR REPLACE INTO reading(started, book, chapter, translation, seconds, furthest, verses)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    parse_rfc3339(&r.started).unwrap_or_else(now),
                    book.info().code,
                    r.chapter,
                    r.translation,
                    r.seconds,
                    r.furthest,
                    r.verses
                ],
            )?;
        }
        for p in &doc.plans {
            tx.execute(
                "INSERT OR REPLACE INTO plans(id, started, finished) VALUES(?1, ?2, ?3)",
                params![
                    p.id,
                    parse_rfc3339(&p.started).unwrap_or_else(now),
                    p.finished.as_deref().and_then(parse_rfc3339)
                ],
            )?;
            for (day, passage) in &p.read {
                tx.execute(
                    "INSERT OR REPLACE INTO plan_reads(plan, day, passage, at) VALUES(?1, ?2, ?3, ?4)",
                    params![p.id, day, passage, now()],
                )?;
            }
        }
        tx.commit()?;
        self.reload()?;
        if !doc.highlights.is_empty() || !doc.bookmarks.is_empty() || !doc.notes.is_empty() {
            self.queued();
        }
        Ok((doc.highlights.len(), doc.notes.len(), doc.bookmarks.len()))
    }

    /// Erases every highlight, note, bookmark, plan and the reading history.
    /// Highlights, bookmarks and notes are cleared through the outbox, so they go from the library
    /// too.
    pub fn erase_all(&mut self) -> Result<()> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let t = now();
        let notes: Vec<String> = tx
            .prepare("SELECT uid FROM items WHERE kind = 'note' AND deleted = 0")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for uid in &notes {
            delete_item(&tx, uid, t)?;
            enqueue_uid(&tx, Kind::Note, uid, t)?;
        }
        for key in self.highlights.keys().copied().filter_map(VerseRef::from_key) {
            put_highlight(&tx, key, None, 0)?;
        }
        for key in self.bookmarks.keys().copied().filter_map(VerseRef::from_key) {
            put_bookmark(&tx, key, None, 0)?;
        }
        tx.execute_batch("DELETE FROM plans; DELETE FROM plan_reads; DELETE FROM reading;")?;
        tx.commit()?;
        let queued = !self.highlights.is_empty() || !self.bookmarks.is_empty() || !notes.is_empty();
        self.reload()?;
        if queued {
            self.queued();
        }
        Ok(())
    }
}

/// Opens study.sqlite as one of its two connections (the app's and the sync engine's): WAL, and
/// waiting out the other's writes rather than failing.
///
/// Every transaction here begins `IMMEDIATE`: one that reads before it writes would otherwise
/// fail with "database is locked" (not wait) when the other connection committed in between.
fn connect(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

fn pending(conn: &Connection) -> Result<usize> {
    Ok(conn.query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get::<_, i64>(0))? as usize)
}

/// The sync engine's side of the cache: what it sends from the outbox, what it does with the
/// answers, and how it applies the change feed. Each function is one transaction.
#[cfg(feature = "sync")]
pub(crate) mod cache {
    use super::*;
    use crate::records::Chat;
    use serde_json::Value;

    /// A change waiting in the outbox, as the mutation's arguments.
    #[derive(Debug, Clone)]
    pub(crate) struct Entry {
        pub id: i64,
        pub kind: Kind,
        pub key: Key,
        /// The mutation that sends it.
        pub name: &'static str,
        /// The mutation's arguments, `baseSeq` included.
        pub args: Value,
        /// A file to upload and attach first: its uid, bytes and SHA-256 (hex).
        pub upload: Option<(String, Vec<u8>, String)>,
        rev: i64,
        pub attempts: i64,
        /// Unix milliseconds before which it isn't tried again (0: now).
        pub next_at: i64,
    }

    /// What an outbox entry is about: a verse for a highlight or bookmark, a uid otherwise.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Key {
        Verse(u32),
        Uid(String),
    }

    pub(crate) fn open(path: &Path) -> Result<Connection> {
        let mut conn = connect(path)?;
        migrate(&mut conn)?;
        Ok(conn)
    }

    pub(crate) fn pending(conn: &Connection) -> Result<usize> {
        super::pending(conn)
    }

    /// The oldest change waiting. Changes go strictly in order, so one backing off holds the rest.
    /// A note or chat is read from the cache as it is now; a note this device made that the
    /// library already has, word for word on the same verse (a second device's migration), is
    /// dropped here instead of uploaded again.
    pub(crate) fn next(conn: &Connection) -> Result<Option<Entry>> {
        use rusqlite::types::Value as Sql;
        loop {
            let row = conn
                .query_row(
                    "SELECT id, kind, key, payload, base_seq, rev, attempts, next_at FROM outbox ORDER BY id LIMIT 1",
                    [],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Sql>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, i64>(4)?,
                            r.get(5)?,
                            r.get(6)?,
                            r.get(7)?,
                        ))
                    },
                )
                .optional()?;
            let Some((id, kind, key, payload, base, rev, attempts, next_at)) = row else { return Ok(None) };
            let kind = Kind::from_name(&kind).with_context(|| format!("unknown outbox kind {kind}"))?;
            let offset = clock_offset_ms(conn)?;
            // A device time in Unix seconds as server milliseconds.
            let made = |t: i64| json!((t * 1000 + offset).max(0));
            let entry =
                |key, name, args, upload| Entry { id, kind, key, name, args, upload, rev, attempts, next_at };
            match (kind, key) {
                (Kind::Highlight | Kind::Bookmark, Sql::Integer(verse)) => {
                    let mut args: Value = serde_json::from_str(&payload)?;
                    args["baseSeq"] = json!(base);
                    if let Some(created) = args["createdAt"].as_i64() {
                        // In this device's milliseconds until now.
                        args["createdAt"] = json!((created + offset).max(0));
                    }
                    return Ok(Some(entry(Key::Verse(verse as u32), kind.mutation().0, args, None)));
                }
                (Kind::Note, Sql::Text(uid)) => {
                    let row = conn
                        .query_row("SELECT deleted, created, seq FROM items WHERE uid = ?1", [&uid], |r| {
                            Ok((r.get::<_, bool>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
                        })
                        .optional()?;
                    let (Some((deleted, created, seq)), Some(item)) = (row, load_item(conn, &uid)?) else {
                        conn.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
                        continue;
                    };
                    if deleted {
                        let args = json!({ "uid": uid, "baseSeq": base });
                        return Ok(Some(entry(Key::Uid(uid), "library/notes:remove", args, None)));
                    }
                    if item.kind != ItemKind::Note {
                        // A sermon or a captured page: only its details are edited on a device.
                        let args = json!({
                            "uid": uid, "title": item.title, "details": item.details,
                            "sermonUid": item.sermon_uid, "offsetMs": item.offset_ms, "baseSeq": base
                        });
                        return Ok(Some(entry(Key::Uid(uid), "library/items:updateDetails", args, None)));
                    }
                    if seq == 0 && base == 0 && already_there(conn, &item)? {
                        let tx = rusqlite::Transaction::new_unchecked(
                            conn,
                            rusqlite::TransactionBehavior::Immediate,
                        )?;
                        tx.execute("DELETE FROM items WHERE uid = ?1", [&uid])?;
                        tx.execute("DELETE FROM item_anchors WHERE uid = ?1", [&uid])?;
                        tx.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
                        tx.commit()?;
                        continue;
                    }
                    let mut args = json!({
                        "uid": uid, "text": item.text.unwrap_or_default(), "anchors": item.anchors, "baseSeq": base
                    });
                    if seq == 0 {
                        args["capturedAt"] = made(created);
                    }
                    return Ok(Some(entry(Key::Uid(uid), "library/notes:upsert", args, None)));
                }
                (Kind::Chat, Sql::Text(uid)) => {
                    let row = conn
                        .query_row(
                            "SELECT deleted, created, seq, title, messages, api, file_uid FROM chats WHERE uid = ?1",
                            [&uid],
                            |r| {
                                Ok((
                                    r.get::<_, bool>(0)?,
                                    r.get::<_, i64>(1)?,
                                    r.get::<_, i64>(2)?,
                                    r.get::<_, String>(3)?,
                                    r.get::<_, String>(4)?,
                                    r.get::<_, String>(5)?,
                                    r.get::<_, Option<String>>(6)?,
                                ))
                            },
                        )
                        .optional()?;
                    let Some((deleted, created, seq, title, messages, api, attached)) = row else {
                        conn.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
                        continue;
                    };
                    if deleted {
                        let args = json!({ "uid": uid, "baseSeq": base });
                        return Ok(Some(entry(Key::Uid(uid), "library/chats:remove", args, None)));
                    }
                    // The conversation is a `chat` file, named by the chat and its bytes, so a retry
                    // attaches the same file and an unchanged one isn't uploaded again.
                    let messages: Value = serde_json::from_str(&messages)?;
                    let bytes = json!({ "messages": messages, "api": api }).to_string().into_bytes();
                    let sha = sha256_hex(&bytes);
                    let file_uid = sha256_hex(format!("{uid}:{sha}").as_bytes());
                    let upload =
                        (attached.as_deref() != Some(&file_uid)).then(|| (file_uid.clone(), bytes, sha));
                    let mut args =
                        json!({ "uid": uid, "title": title, "fileUid": file_uid, "baseSeq": base });
                    if seq == 0 {
                        args["createdAt"] = made(created);
                    }
                    return Ok(Some(entry(Key::Uid(uid), "library/chats:save", args, upload)));
                }
                (kind, key) => bail!("outbox entry {id} for a {kind:?} has the key {key:?}"),
            }
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(bytes))
    }

    /// Whether the library already has a note with this one's text and anchors.
    fn already_there(conn: &Connection, item: &Item) -> Result<bool> {
        let mut st = conn.prepare(
            "SELECT record FROM items WHERE kind = 'note' AND deleted = 0 AND seq > 0 AND uid != ?1",
        )?;
        let records: Vec<String> = st.query_map([&item.uid], |r| r.get(0))?.collect::<Result<_, _>>()?;
        Ok(records
            .iter()
            .filter_map(|r| serde_json::from_str::<Item>(r).ok())
            .any(|other| other.text == item.text && other.anchors == item.anchors))
    }

    /// A chat's file was uploaded and attached as `file_uid`.
    pub(crate) fn attached(conn: &Connection, chat: &str, file_uid: &str, url: &str) -> Result<()> {
        conn.execute(
            "UPDATE chats SET file_uid = ?2, url = ?3 WHERE uid = ?1",
            params![chat, file_uid, url],
        )?;
        Ok(())
    }

    /// When the next change that is backing off falls due, if any is.
    pub(crate) fn wake_at(conn: &Connection) -> Result<Option<i64>> {
        Ok(conn.query_row("SELECT MIN(next_at) FROM outbox WHERE next_at > 0", [], |r| r.get(0))?)
    }

    /// The last error a waiting change got, if any change is failing.
    pub(crate) fn last_error(conn: &Connection) -> Result<Option<String>> {
        Ok(conn
            .query_row(
                "SELECT last_error FROM outbox WHERE last_error IS NOT NULL ORDER BY id LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The server took the change as `seq`. If it was edited again meanwhile, the newer edit stays
    /// queued, now against `seq`; if not, a note or chat takes `record`, the server's copy as the
    /// feed sends it, so this device holds what every other one gets (its times included).
    pub(crate) fn sent(
        conn: &mut Connection,
        e: &Entry,
        seq: u64,
        record: Option<&FeedRecord>,
    ) -> Result<()> {
        let offset = clock_offset_ms(conn)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let done = tx.execute("DELETE FROM outbox WHERE id = ?1 AND rev = ?2", params![e.id, e.rev])? > 0;
        if !done {
            tx.execute("UPDATE outbox SET base_seq = ?2 WHERE id = ?1", params![e.id, seq as i64])?;
        }
        match (&e.key, record) {
            (Key::Uid(_), Some(record)) if done => put_server(&tx, record, offset)?,
            (Key::Verse(verse), _) => {
                tx.execute(
                    &format!("UPDATE {} SET seq = MAX(seq, ?2) WHERE verse = ?1", e.kind.table()),
                    params![verse, seq as i64],
                )?;
            }
            (Key::Uid(uid), _) => {
                tx.execute(
                    &format!("UPDATE {} SET seq = MAX(seq, ?2) WHERE uid = ?1", e.kind.table()),
                    params![uid, seq as i64],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Writes the server's copy of a note or chat (as the feed or a write's answer sends it) into
    /// the cache, dated by the server's times. Highlights and bookmarks go through `write_server`.
    fn put_server(tx: &Connection, record: &FeedRecord, clock_offset_ms: i64) -> Result<()> {
        match record {
            FeedRecord::Item(item) => {
                let created = local_time(Some(item.captured_at), clock_offset_ms).unwrap_or_else(now);
                let updated = local_time(item.updated_at, clock_offset_ms).unwrap_or_else(now);
                put_item(tx, item, created, updated)
            }
            FeedRecord::Chat(chat) => write_chat(tx, chat, clock_offset_ms),
            FeedRecord::Highlight(_) | FeedRecord::Bookmark(_) => Ok(()),
        }
    }

    /// The server kept its own copy of a highlight or bookmark because another device changed it
    /// after this one's copy: the change is done and the cache takes the server's copy, `record`.
    /// Returns whether that was a real conflict, recorded for the UI.
    ///
    /// Two cases aren't: the server already has this edit (a write that reached it before the
    /// connection broke, or the same highlight made on both devices), and a record made here, with
    /// no copy, over a deletion this device never saw, which is sent again against the tombstone.
    pub(crate) fn conflicted(conn: &mut Connection, e: &Entry, record: &FeedRecord) -> Result<bool> {
        let Key::Verse(verse) = &e.key else { anyhow::bail!("{} is kept by uid", e.kind.name()) };
        let (seq, created, theirs) = match record {
            FeedRecord::Highlight(h) => {
                (h.seq, h.created_at, h.deleted_at.is_none().then(|| json!(h.color.name())))
            }
            FeedRecord::Bookmark(b) => (b.seq, b.created_at, b.deleted_at.is_none().then(|| json!(b.label))),
            _ => anyhow::bail!("a {} answer that isn't one", e.kind.name()),
        };
        let theirs = &theirs.unwrap_or(Value::Null);
        let offset = clock_offset_ms(conn)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (rev, payload, base): (i64, String, i64) =
            tx.query_row("SELECT rev, payload, base_seq FROM outbox WHERE id = ?1", [e.id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        if rev != e.rev {
            // Edited again while this was in flight: the next answer settles the newer edit.
            return Ok(false);
        }
        let payload: Value = serde_json::from_str(&payload)?;
        let field = e.kind.mutation().1;
        let mine = &payload[field];
        if base == 0 && theirs.is_null() && !mine.is_null() {
            tx.execute("UPDATE outbox SET base_seq = ?2 WHERE id = ?1", params![e.id, seq as i64])?;
            tx.commit()?;
            return Ok(false);
        }
        tx.execute("DELETE FROM outbox WHERE id = ?1", [e.id])?;
        let created = local_time(created, offset);
        write_server(&tx, e.kind, *verse, seq, theirs, created)?;
        // Base 0 included: an edit made before the feed first caught up has no copy either, and
        // losing it silently is what conflicts exist to prevent. Migration lands here too, and
        // says so when the server's copy differs.
        let real = mine != theirs;
        if real {
            record_conflict(&tx, e.kind, *verse, &mine.to_string(), &theirs.to_string())?;
        }
        tx.commit()?;
        Ok(real)
    }

    /// The server kept its own copy of a note or chat (`record`) because another device changed
    /// it after this one's copy. A note edit isn't lost: the server kept it as a conflict copy
    /// (`copy`), which this device's note becomes, and the conflict is recorded for the UI. A
    /// chat's is lost, as for a highlight, and not recorded (a conflict names a verse). Returns
    /// whether it was recorded.
    pub(crate) fn conflicted_by_uid(
        conn: &mut Connection,
        e: &Entry,
        record: &FeedRecord,
        copy: Option<&FeedRecord>,
    ) -> Result<bool> {
        let Key::Uid(uid) = &e.key else { anyhow::bail!("{} is kept by verse", e.kind.name()) };
        let offset = clock_offset_ms(conn)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let rev: i64 = tx.query_row("SELECT rev FROM outbox WHERE id = ?1", [e.id], |r| r.get(0))?;
        if e.kind != Kind::Note {
            if rev != e.rev {
                return Ok(false);
            }
            tx.execute("DELETE FROM outbox WHERE id = ?1", [e.id])?;
            put_server(&tx, record, offset)?;
            tx.commit()?;
            return Ok(false);
        }
        let mine = load_item(&tx, uid)?;
        let removing = e.name == "library/notes:remove";
        match (copy, &mine) {
            (Some(FeedRecord::Item(copy)), Some(mine)) if !removing => {
                tx.execute("DELETE FROM items WHERE uid = ?1", [&copy.uid])?;
                if rev != e.rev {
                    // Edited again in flight: the newer text goes on as an edit of the copy.
                    let updated: i64 =
                        tx.query_row("SELECT updated FROM items WHERE uid = ?1", [uid], |r| r.get(0))?;
                    let created = local_time(Some(copy.captured_at), offset).unwrap_or_else(now);
                    let edited = Item {
                        title: mine.title.clone(),
                        text: mine.text.clone(),
                        anchors: mine.anchors.clone(),
                        ..(**copy).clone()
                    };
                    put_item(&tx, &edited, created, updated)?;
                    tx.execute(
                        "UPDATE outbox SET key = ?2, base_seq = ?3 WHERE id = ?1",
                        params![e.id, copy.uid, copy.seq as i64],
                    )?;
                } else {
                    put_server(&tx, &FeedRecord::Item(copy.clone()), offset)?;
                    tx.execute("DELETE FROM outbox WHERE id = ?1", [e.id])?;
                }
            }
            _ => {
                tx.execute("DELETE FROM outbox WHERE id = ?1", [e.id])?;
            }
        }
        put_server(&tx, record, offset)?;
        let theirs = match record {
            FeedRecord::Item(i) => Some(&**i),
            _ => None,
        };
        let text = |i: Option<&Item>| match i {
            Some(i) if i.deleted_at.is_none() => json!(i.text.as_deref().unwrap_or("")),
            _ => Value::Null,
        };
        let mine_text = if removing { Value::Null } else { text(mine.as_ref()) };
        let theirs_text = text(theirs);
        let verse = mine.as_ref().and_then(|m| m.anchors.first()).map(|a| a.from.0);
        let real = match verse {
            Some(v) if mine_text != theirs_text => {
                record_conflict(&tx, e.kind, v, &mine_text.to_string(), &theirs_text.to_string())?;
                true
            }
            _ => false,
        };
        tx.commit()?;
        Ok(real)
    }

    fn record_conflict(tx: &Connection, kind: Kind, verse: u32, mine: &str, theirs: &str) -> Result<()> {
        tx.execute(
            "INSERT INTO conflicts(kind, key, mine, theirs, at) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![kind.name(), verse, mine, theirs, now()],
        )?;
        Ok(())
    }

    /// The server refused the change: show `error`, and try it again at `next_at`.
    pub(crate) fn failed(conn: &Connection, e: &Entry, error: &str, next_at: i64) -> Result<()> {
        conn.execute(
            "UPDATE outbox SET attempts = attempts + 1, last_error = ?2, next_at = ?3 WHERE id = ?1",
            params![e.id, error, next_at],
        )?;
        Ok(())
    }

    /// Retry: everything waiting is due now.
    pub(crate) fn retry_all(conn: &Connection) -> Result<()> {
        conn.execute("UPDATE outbox SET next_at = 0, last_error = NULL", [])?;
        Ok(())
    }

    /// The cursor: the `seq` of the last feed page applied.
    pub(crate) fn cursor(conn: &Connection) -> Result<u64> {
        Ok(state(conn, "cursor")?.unwrap_or(0) as u64)
    }

    pub(crate) fn clock_offset_ms(conn: &Connection) -> Result<i64> {
        Ok(state(conn, "clock_offset_ms")?.unwrap_or(0))
    }

    /// The device's clock offset from the server's, measured from an uncached answer.
    pub(crate) fn set_clock_offset_ms(conn: &mut Connection, ms: i64) -> Result<()> {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        set_state(&tx, "clock_offset_ms", ms)?;
        tx.commit()?;
        Ok(())
    }

    fn state(conn: &Connection, key: &str) -> Result<Option<i64>> {
        Ok(conn.query_row("SELECT value FROM sync_state WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    fn set_state(tx: &rusqlite::Transaction, key: &str, value: i64) -> Result<()> {
        tx.execute(
            "INSERT INTO sync_state(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Applies one page of the change feed and moves the cursor to `seq`, in one transaction. A
    /// record with a change of this device's still waiting keeps the local value (the outbox entry
    /// settles it), but the cache learns the server's `seq`, so that entry conflicts if another
    /// device moved the record past it. Returns how many records changed what the app shows.
    pub(crate) fn apply_page(conn: &mut Connection, records: &[FeedRecord], seq: u64) -> Result<usize> {
        let clock_offset_ms = clock_offset_ms(conn)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut changed = 0;
        for record in records {
            let (kind, verse, rseq, created, value) = match record {
                FeedRecord::Highlight(h) => (
                    Kind::Highlight,
                    h.verse.0,
                    h.seq,
                    h.created_at,
                    h.deleted_at.is_none().then(|| json!(h.color.name())),
                ),
                FeedRecord::Bookmark(b) => (
                    Kind::Bookmark,
                    b.verse.0,
                    b.seq,
                    b.created_at,
                    b.deleted_at.is_none().then(|| json!(b.label)),
                ),
                FeedRecord::Item(item) => {
                    if newer_by_uid(&tx, Kind::Note, &item.uid, item.seq)? {
                        put_server(&tx, record, clock_offset_ms)?;
                        changed += 1;
                    }
                    continue;
                }
                FeedRecord::Chat(chat) => {
                    if newer_by_uid(&tx, Kind::Chat, &chat.uid, chat.seq)? {
                        put_server(&tx, record, clock_offset_ms)?;
                        changed += 1;
                    }
                    continue;
                }
            };
            let value = value.unwrap_or(Value::Null);
            let waiting: Option<i64> = tx
                .query_row(
                    "SELECT base_seq FROM outbox WHERE kind = ?1 AND key = ?2",
                    params![kind.name(), verse],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(base) = waiting {
                // A record made here without a copy (base 0) over a deletion isn't a conflict:
                // there was nothing to see.
                if base == 0 && value.is_null() {
                    tx.execute(
                        "UPDATE outbox SET base_seq = ?3 WHERE kind = ?1 AND key = ?2",
                        params![kind.name(), verse, rseq as i64],
                    )?;
                }
                tx.execute(
                    &format!("UPDATE {} SET seq = MAX(seq, ?2) WHERE verse = ?1", kind.table()),
                    params![verse, rseq as i64],
                )?;
                continue;
            }
            let have: Option<i64> = tx
                .query_row(&format!("SELECT seq FROM {} WHERE verse = ?1", kind.table()), [verse], |r| {
                    r.get(0)
                })
                .optional()?;
            if have.is_none_or(|have| (rseq as i64) > have) {
                write_server(&tx, kind, verse, rseq, &value, local_time(created, clock_offset_ms))?;
                changed += 1;
            }
        }
        set_state(&tx, "cursor", seq as i64)?;
        tx.commit()?;
        Ok(changed)
    }

    /// Whether the feed's copy of the note or chat `uid` (at `seq`) replaces the cache's. One with
    /// a change of this device's waiting keeps it and only learns the server's `seq`.
    fn newer_by_uid(tx: &Connection, kind: Kind, uid: &str, seq: u64) -> Result<bool> {
        let table = kind.table();
        let waiting = tx.query_row(
            "SELECT 1 FROM outbox WHERE kind = ?1 AND key = ?2",
            params![kind.name(), uid],
            |_| Ok(()),
        );
        if waiting.optional()?.is_some() {
            tx.execute(
                &format!("UPDATE {table} SET seq = MAX(seq, ?2) WHERE uid = ?1"),
                params![uid, seq as i64],
            )?;
            return Ok(false);
        }
        let have: Option<i64> = tx
            .query_row(&format!("SELECT seq FROM {table} WHERE uid = ?1"), [uid], |r| r.get(0))
            .optional()?;
        Ok(have.is_none_or(|have| seq as i64 > have))
    }

    /// Writes the server's copy of a chat. Its conversation comes later, when the engine fetches
    /// the file, unless the file is the one already here.
    fn write_chat(tx: &Connection, c: &Chat, clock_offset_ms: i64) -> Result<()> {
        let t = now();
        let created = local_time(Some(c.created_at), clock_offset_ms).unwrap_or(t);
        let updated = local_time(c.updated_at, clock_offset_ms).unwrap_or(t);
        let ord: i64 = tx.query_row("SELECT IFNULL(MAX(ord), 0) + 1 FROM chats", [], |r| r.get(0))?;
        tx.execute(
            "INSERT INTO chats(title, created, updated, messages, api, ord, uid, seq, deleted, file_uid, url, fetched)
             VALUES(?1, ?2, ?3, '[]', '', ?4, ?5, ?6, ?7, ?8, ?9, 0)
             ON CONFLICT(uid) DO UPDATE SET title = excluded.title, created = excluded.created, updated = excluded.updated,
               ord = excluded.ord, seq = excluded.seq, deleted = excluded.deleted,
               fetched = CASE WHEN file_uid IS excluded.file_uid THEN fetched ELSE 0 END,
               file_uid = excluded.file_uid, url = excluded.url",
            params![
                c.title,
                created,
                updated,
                ord,
                c.uid,
                c.seq as i64,
                c.deleted_at.is_some() as i64,
                c.file_uid,
                c.url
            ],
        )?;
        Ok(())
    }

    /// A file the cache wants: a chat's conversation, or an item's text.
    #[derive(Debug, Clone)]
    pub(crate) struct Fetch {
        pub chat: bool,
        pub uid: String,
        pub url: String,
    }

    /// Files the cache doesn't have yet. Text files are fetched as soon as they're known (they're
    /// small, and offline search reads them); conversations too, so Recents opens offline.
    pub(crate) fn to_fetch(conn: &Connection) -> Result<Vec<Fetch>> {
        let mut out = Vec::new();
        for (chat, sql) in [
            (true, "SELECT uid, url FROM chats WHERE deleted = 0 AND fetched = 0 AND IFNULL(url, '') != ''"),
            (false, "SELECT uid, url FROM files WHERE role = 'text' AND path = ''"),
        ] {
            let mut st = conn.prepare_cached(sql)?;
            let rows = st.query_map([], |r| Ok(Fetch { chat, uid: r.get(0)?, url: r.get(1)? }))?;
            out.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        Ok(out)
    }

    /// Keeps a fetched file: a conversation in its chat, text under `files_dir`.
    pub(crate) fn fetched(conn: &Connection, f: &Fetch, bytes: &[u8], files_dir: &Path) -> Result<()> {
        if f.chat {
            let file: Value = serde_json::from_slice(bytes).context("not a conversation")?;
            let messages: Vec<crate::chat::Message> = serde_json::from_value(file["messages"].clone())?;
            conn.execute(
                "UPDATE chats SET messages = ?3, api = ?4, fetched = 1 WHERE uid = ?1 AND url = ?2",
                params![f.uid, f.url, serde_json::to_string(&messages)?, file["api"].as_str().unwrap_or("")],
            )?;
        } else {
            std::fs::create_dir_all(files_dir)?;
            let path = files_dir.join(&f.uid);
            std::fs::write(&path, bytes)?;
            conn.execute(
                "UPDATE files SET path = ?2 WHERE uid = ?1",
                params![f.uid, path.to_string_lossy()],
            )?;
        }
        Ok(())
    }

    /// The server dropped tombstones this device hadn't seen: forget the cache (keeping records
    /// with a change still waiting, which the outbox settles) and pull the library from 0.
    pub(crate) fn resync(conn: &mut Connection) -> Result<()> {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for kind in [Kind::Highlight, Kind::Bookmark] {
            tx.execute(
                &format!(
                    "DELETE FROM {} WHERE verse NOT IN (SELECT key FROM outbox WHERE kind = ?1)",
                    kind.table()
                ),
                [kind.name()],
            )?;
        }
        for kind in [Kind::Note, Kind::Chat] {
            tx.execute(
                &format!(
                    "DELETE FROM {} WHERE uid NOT IN (SELECT key FROM outbox WHERE kind = ?1)",
                    kind.table()
                ),
                [kind.name()],
            )?;
        }
        tx.execute_batch(
            "DELETE FROM item_anchors WHERE uid NOT IN (SELECT uid FROM items);
             DELETE FROM files WHERE owner NOT IN (SELECT uid FROM items);",
        )?;
        set_state(&tx, "cursor", 0)?;
        tx.commit()?;
        Ok(())
    }

    /// A server time in milliseconds as this device's Unix seconds: the server's second, moved by
    /// the offset in whole seconds. Rounding the offset, not the moved time, keeps every device on
    /// the same second for the same stamp: measured offsets jitter by tens of milliseconds, which
    /// moved a stamp near half a second across the boundary on one device and not another.
    pub(crate) fn local_time(server_ms: Option<u64>, clock_offset_ms: i64) -> Option<i64> {
        server_ms.map(|ms| (ms as i64).div_euclid(1000) - (clock_offset_ms + 500).div_euclid(1000))
    }

    #[cfg(test)]
    #[test]
    fn one_stamp_is_the_same_second_whatever_each_device_measured() {
        // The Mac prover's case: a stamp at .613 s, offsets 95 and 136 ms.
        let stamp = Some(1_792_505_303_613);
        assert_eq!(local_time(stamp, 95), Some(1_792_505_303));
        assert_eq!(local_time(stamp, 136), local_time(stamp, 95));
        assert_eq!(local_time(stamp, -92), local_time(stamp, 95));
        // A device whose clock is 5 s fast sees it 5 s later on its own clock.
        assert_eq!(local_time(stamp, -5_000), Some(1_792_505_308));
    }

    /// Writes the server's copy of a record into the cache: a value, or a tombstone for `null`
    /// (kept, with its `seq`, so a later edit here is made against it). `created` is the server's
    /// creation time, which stands; without one (a record older than the field) it is dated now
    /// when new.
    fn write_server(
        tx: &rusqlite::Transaction,
        kind: Kind,
        verse: u32,
        seq: u64,
        value: &Value,
        created: Option<i64>,
    ) -> Result<()> {
        let t = now();
        let (column, stored): (&str, rusqlite::types::Value) = match kind {
            Kind::Highlight => {
                let color =
                    value.as_str().and_then(|n| HighlightColor::ALL.into_iter().find(|c| c.name() == n));
                ("color", color.map_or(0, HighlightColor::to_i64).into())
            }
            Kind::Bookmark => ("label", value.as_str().unwrap_or("").to_string().into()),
            Kind::Note | Kind::Chat => unreachable!("{kind:?} is kept by uid"),
        };
        let deleted = value.is_null() as i64;
        tx.execute(
            &format!(
                "INSERT INTO {table}(verse, {column}, created, updated, seq, deleted) VALUES(?1, ?2, ?3, ?7, ?4, ?5)
                 ON CONFLICT(verse) DO UPDATE SET {column} = excluded.{column},
                   created = CASE WHEN ?6 IS NOT NULL OR deleted = 1 THEN excluded.created ELSE created END,
                   updated = excluded.updated, seq = excluded.seq, deleted = excluded.deleted",
                table = kind.table()
            ),
            params![verse, stored, created.unwrap_or(t), seq as i64, deleted, created, t],
        )?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct ExportDoc {
    app: String,
    format: u32,
    exported_at: String,
    translation: String,
    #[serde(default)]
    highlights: Vec<ExportHighlight>,
    #[serde(default)]
    notes: Vec<ExportNote>,
    #[serde(default)]
    bookmarks: Vec<ExportBookmark>,
    /// Reading sessions and plans, when the export includes history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<ExportReading>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    plans: Vec<ExportPlan>,
}

#[derive(Serialize, Deserialize)]
struct ExportReading {
    started: String,
    book: String,
    chapter: u8,
    translation: String,
    seconds: u32,
    furthest: u8,
    verses: u8,
}

#[derive(Serialize, Deserialize)]
struct ExportPlan {
    id: String,
    started: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finished: Option<String>,
    /// Passages ticked, as (day, index).
    read: Vec<(u16, u8)>,
}

/// What an export carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Include {
    pub highlights: bool,
    pub notes: bool,
    pub bookmarks: bool,
    /// Reading history and plans.
    pub history: bool,
}

impl Include {
    /// Highlights, notes and bookmarks.
    pub const STUDY: Include = Include { highlights: true, notes: true, bookmarks: true, history: false };
    pub const EVERYTHING: Include = Include { highlights: true, notes: true, bookmarks: true, history: true };

    pub const fn default_none() -> Include {
        Include { highlights: false, notes: false, bookmarks: false, history: false }
    }
}

#[derive(Serialize, Deserialize)]
struct ExportVerse {
    reference: String,
    book: String,
    chapter: u8,
    verse: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ExportHighlight {
    #[serde(flatten)]
    verse: ExportVerse,
    color: HighlightColor,
    created: String,
}

#[derive(Serialize, Deserialize)]
struct ExportNote {
    #[serde(flatten)]
    verse: ExportVerse,
    note: String,
    created: String,
    updated: String,
    /// The note item's id, so importing a backup again updates it rather than adding another.
    /// Absent in exports made before notes became items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    uid: Option<String>,
    /// For a conflict copy, the uid of the note it collided with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conflict_of: Option<String>,
}

/// A live note item as export lists it: its verse (the first anchor's), text, times, and ids.
struct AnyNote {
    verse: VerseRef,
    uid: String,
    body: String,
    created: i64,
    updated: i64,
    conflict_of: Option<String>,
}

/// Every live note item, several to a verse allowed, in verse order then oldest first.
fn all_notes(conn: &Connection) -> Result<Vec<AnyNote>> {
    let mut st = conn.prepare(
        "SELECT uid, created, updated, record FROM items WHERE kind = 'note' AND deleted = 0 ORDER BY created, uid",
    )?;
    let rows = st.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (uid, created, updated, record) = row?;
        let Ok(item) = serde_json::from_str::<Item>(&record) else { continue };
        let Some(verse) = item.anchors.first().and_then(|a| verse_of(a.from.0 as i64)) else { continue };
        let body = item.text.unwrap_or_default();
        out.push(AnyNote { verse, uid, body, created, updated, conflict_of: item.conflict_of });
    }
    out.sort_by_key(|n| n.verse);
    Ok(out)
}

/// Imports one exported note; returns the uid it was written to, if it was. Its uid, when this
/// cache has that item, updates it; a note already on the verse word for word is skipped; an
/// export from before uids overwrites the verse's own note with its first note on that verse
/// (`legacy_first`); anything else becomes another note item, conflict copies included.
fn import_note(
    tx: &Connection,
    own: Option<&Note>,
    v: VerseRef,
    n: &ExportNote,
    legacy_first: bool,
) -> Result<Option<String>> {
    let created = parse_rfc3339(&n.created).unwrap_or_else(now);
    let updated = parse_rfc3339(&n.updated).unwrap_or_else(now);
    if n.note.trim().is_empty() {
        return Ok(None);
    }
    if let Some(mut item) = n.uid.as_deref().map(|u| load_item(tx, u)).transpose()?.flatten() {
        if item.deleted_at.is_none() && item.text.as_deref() == Some(n.note.as_str()) {
            return Ok(None);
        }
        set_note_text(&mut item, &n.note);
        item.deleted_at = None;
        put_item(tx, &item, created, updated)?;
        return Ok(Some(item.uid));
    }
    if all_notes(tx)?.iter().any(|o| o.verse == v && o.body == n.note) {
        return Ok(None);
    }
    if n.uid.is_none() && legacy_first {
        return Ok(write_note(tx, own, v, &n.note, created, updated)?.map(|note| note.uid));
    }
    let uid = n.uid.clone().unwrap_or_else(new_uid);
    let mut item = note_item(&uid, VerseId::from(v), &n.note, created);
    item.conflict_of = n.conflict_of.clone();
    put_item(tx, &item, created, updated)?;
    Ok(Some(uid))
}

#[derive(Serialize, Deserialize)]
struct ExportBookmark {
    #[serde(flatten)]
    verse: ExportVerse,
    label: String,
    created: String,
}

/// Unix seconds → `YYYY-MM-DDTHH:MM:SSZ` (proleptic Gregorian, UTC).
pub fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let days = days_from_civil(num(0..4)?, num(5..7)? as u32, num(8..10)? as u32);
    Some(days * 86_400 + num(11..13)? * 3600 + num(14..16)? * 60 + num(17..19)?)
}

// Howard Hinnant's date algorithms.
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod chat_tests {
    use super::*;
    use crate::chat::Message;

    #[test]
    fn chats_save_continue_list_and_delete() {
        let mut s = StudyStore::open_in_memory().unwrap();
        let first =
            s.save_chat(None, "Who was Nicodemus?", &[Message::user("Who was Nicodemus?")], "[1]").unwrap();
        let second = s.save_chat(None, "Born again", &[Message::user("Born again?")], "[2]").unwrap();
        assert_ne!(first, second);
        // Continuing the first moves it to the top and keeps its id.
        let messages = [Message::user("Who was Nicodemus?"), Message::assistant("A Pharisee {{John 3:1}}.")];
        assert_eq!(s.save_chat(Some(first), "Who was Nicodemus?", &messages, "[1,2]").unwrap(), first);
        let ids: Vec<i64> = s.chats().iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![first, second]);
        let saved = s.chat(first).unwrap();
        assert_eq!(saved.messages, messages);
        assert_eq!(saved.api, "[1,2]");
        s.delete_chat(first).unwrap();
        assert!(s.chat(first).is_none());
        assert_eq!(s.chats().len(), 1);
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    #[test]
    fn plans_start_tick_and_stop() {
        let mut s = StudyStore::open_in_memory().unwrap();
        s.start_plan("john-21", 1_000).unwrap();
        s.set_plan_read("john-21", 1, 0, true).unwrap();
        s.set_plan_read("john-21", 2, 0, true).unwrap();
        s.set_plan_read("john-21", 2, 0, false).unwrap();
        assert_eq!(s.plans()[0].id, "john-21");
        assert_eq!(s.plan_reads("john-21").into_iter().collect::<Vec<_>>(), vec![(1, 0)]);
        // Restarting forgets progress.
        s.start_plan("john-21", 2_000).unwrap();
        assert!(s.plan_reads("john-21").is_empty());
        s.finish_plan("john-21", Some(3_000)).unwrap();
        assert_eq!(s.plans()[0].finished, Some(3_000));
        s.stop_plan("john-21").unwrap();
        assert!(s.plans().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jn316() -> VerseRef {
        VerseRef::new(BookId(42), 3, 16)
    }

    #[test]
    fn reading_sessions() {
        let s = StudyStore::open_in_memory().unwrap();
        let mut session = ReadingSession {
            started: 1_000,
            book: BookId(42),
            chapter: 3,
            translation: "kjv".into(),
            seconds: 30,
            furthest: 10,
            verses: 36,
        };
        s.record_reading(&session).unwrap();
        session.seconds = 400;
        session.furthest = 36;
        s.record_reading(&session).unwrap();
        s.record_reading(&ReadingSession { started: 2_000, chapter: 4, furthest: 3, ..session.clone() })
            .unwrap();
        let all = s.reading_since(0);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].chapter, 4, "newest first");
        assert_eq!(all[1].seconds, 400, "a session is saved in place");
        assert!(all[1].finished() && !all[0].finished());
        assert_eq!(s.reading_since(1_500).len(), 1);
        s.clear_reading(1_500).unwrap();
        assert_eq!(s.reading_since(0).len(), 1);
    }

    #[test]
    fn crud_and_persistence() {
        let dir = std::env::temp_dir().join(format!("linger-study-{}", std::process::id()));
        let path = dir.join("study.sqlite");
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut s = StudyStore::open(&path).unwrap();
            let g = s.generation();
            s.set_highlight(jn316(), Some(HighlightColor::Yellow)).unwrap();
            s.set_highlight(jn316(), Some(HighlightColor::Green)).unwrap();
            s.set_note(jn316(), "God's love").unwrap();
            assert!(s.toggle_bookmark(VerseRef::new(BookId(0), 1, 1)).unwrap());
            s.set_setting("theme", "dark").unwrap();
            assert!(s.generation() > g);
        }
        let mut s = StudyStore::open(&path).unwrap();
        assert_eq!(s.highlight(jn316()), Some(HighlightColor::Green));
        assert_eq!(s.note(jn316()).unwrap().body, "God's love");
        assert!(s.is_bookmarked(VerseRef::new(BookId(0), 1, 1)));
        assert_eq!(s.setting("theme").as_deref(), Some("dark"));
        assert_eq!(s.counts(), (1, 1, 1));
        s.set_note(jn316(), "  ").unwrap();
        s.set_highlight(jn316(), None).unwrap();
        assert!(!s.toggle_bookmark(VerseRef::new(BookId(0), 1, 1)).unwrap());
        assert_eq!(s.counts(), (0, 0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_round_trip_and_markdown() {
        let mut a = StudyStore::open_in_memory().unwrap();
        a.set_highlight(jn316(), Some(HighlightColor::Pink)).unwrap();
        a.set_note(jn316(), "line one\nline two").unwrap();
        a.set_bookmark(VerseRef::new(BookId(18), 23, 1), Some("Shepherd")).unwrap();
        let text = |v: VerseRef| Some(format!("text of {v}"));
        let json = a.export_json("KJV", text).unwrap();
        assert!(json.contains("\"reference\": \"John 3:16\""));
        assert!(json.contains("\"color\": \"pink\""));

        let mut b = StudyStore::open_in_memory().unwrap();
        assert_eq!(b.import_json(&json).unwrap(), (1, 1, 1));
        assert_eq!(b.highlight(jn316()), Some(HighlightColor::Pink));
        assert_eq!(b.note(jn316()).unwrap().body, "line one\nline two");
        assert_eq!(b.bookmark(VerseRef::new(BookId(18), 23, 1)).unwrap().label, "Shepherd");
        assert_eq!(b.note(jn316()).unwrap().created, a.note(jn316()).unwrap().created);

        let md = b.export_markdown("KJV", text);
        assert!(md.contains("## Bookmarks"));
        assert!(md.contains("- **Psalm 23:1** — Shepherd"));
        assert!(md.contains("- **John 3:16** (pink) text of John 3:16"));
        assert!(md.contains("### John 3:16\n\n> text of John 3:16\n\nline one\nline two"));
        assert!(b.import_json("{}").is_err());
        // History and plans travel when asked for, and erasing clears everything.
        a.record_reading(&ReadingSession {
            started: 1_000,
            book: BookId(42),
            chapter: 3,
            translation: "kjv".into(),
            seconds: 300,
            furthest: 36,
            verses: 36,
        })
        .unwrap();
        a.start_plan("john-21", 900).unwrap();
        a.set_plan_read("john-21", 1, 0, true).unwrap();
        let all = a.export_json_with("KJV", text, Include::EVERYTHING).unwrap();
        let only_notes =
            a.export_json_with("KJV", text, Include { notes: true, ..Include::default_none() }).unwrap();
        assert!(!only_notes.contains("\"highlights\": [\n"), "{only_notes}");
        let mut c = StudyStore::open_in_memory().unwrap();
        c.import_json(&all).unwrap();
        assert_eq!(c.reading_since(0).len(), 1);
        assert_eq!(c.plan_reads("john-21").len(), 1);
        c.erase_all().unwrap();
        assert_eq!((c.counts(), c.reading_since(0).len(), c.plans().len()), ((0, 0, 0), 0, 0));
    }

    #[test]
    fn dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_791_158_400), "2026-10-05T00:00:00Z");
        for t in [0, 951_782_400, 1_791_158_399, 4_102_444_800] {
            assert_eq!(parse_rfc3339(&rfc3339(t)), Some(t));
        }
    }
}
