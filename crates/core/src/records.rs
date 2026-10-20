//! The synced records: the one definition of every shape a device and Convex exchange, and the
//! generators that publish it.
//!
//! `cargo xtask schema` writes `server/shared/records.ts`, with an Effect `Schema` and a Convex
//! validator for each record, and `server/convex/lib/canon.json`, the verses in every chapter of
//! the canonical grid. Tests here fail when either file is out of date, so the server can't drift
//! from this file, and the FFI and the wasm bindings expose these same types.
//!
//! Records serialize as camelCase JSON. An absent optional field is left out, never `null`.

use std::fmt::{self, Write as _};

use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::books::{BOOKS, BookId, ChapterIdx, VerseRef};
use crate::contract::sorted;
use crate::study::HighlightColor;

/// A verse on the canonical 66-book grid as one integer, `book × 1,000,000 + chapter × 1,000 +
/// verse`, with books numbered from 0 (Genesis): John 3:16 is `42_003_016`. Ids sort in canon
/// order, so a range is two numbers, and they read as references in a database. This is the one
/// encoding on the wire and in the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct VerseId(pub u32);

impl VerseId {
    pub fn new(book: BookId, chapter: u8, verse: u8) -> Self {
        Self(book.index() as u32 * 1_000_000 + chapter as u32 * 1_000 + verse as u32)
    }

    /// The verse, if it exists on the grid: a real book, a chapter it has, and a verse in that
    /// chapter's KJV versification.
    pub fn verse_ref(self) -> Option<VerseRef> {
        let book = BookId::new((self.0 / 1_000_000) as usize)?;
        let chapter = u8::try_from((self.0 / 1_000) % 1_000).ok()?;
        let verse = u8::try_from(self.0 % 1_000).ok()?;
        let idx = ChapterIdx::new(book, chapter)?;
        (verse >= 1 && verse <= idx.kjv_verses()).then(|| VerseRef::new(book, chapter, verse))
    }

    pub fn is_valid(self) -> bool {
        self.verse_ref().is_some()
    }
}

impl From<VerseRef> for VerseId {
    fn from(v: VerseRef) -> Self {
        Self::new(v.book, v.chapter, v.verse)
    }
}

impl fmt::Display for VerseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.verse_ref() {
            Some(v) => write!(f, "{v}"),
            None => write!(f, "#{}", self.0),
        }
    }
}

/// A highlighted verse: one per verse, last writer wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Highlight {
    /// The record's id, made by the client: for a highlight, derived from the verse.
    pub uid: String,
    /// The per-user sequence number of the write that last changed this record.
    pub seq: u64,
    /// When the record was deleted (a tombstone other devices drop), in server milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<u64>,
    /// When the record was made, in server milliseconds: the device's own time on a first upload
    /// (so a migrated library keeps its dates), else the server's; kept on update, and set again
    /// when a tombstone comes back. Absent only on records written before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    pub verse: VerseId,
    pub color: HighlightColor,
}

/// A bookmarked verse with its label: one per verse, last writer wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    /// The record's id, made by the client: for a bookmark, derived from the verse.
    pub uid: String,
    /// The per-user sequence number of the write that last changed this record.
    pub seq: u64,
    /// When the record was deleted (a tombstone other devices drop), in server milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<u64>,
    /// When the record was made, in server milliseconds: the device's own time on a first upload
    /// (so a migrated library keeps its dates), else the server's; kept on update, and set again
    /// when a tombstone comes back. Absent only on records written before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    pub verse: VerseId,
    pub label: String,
}

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    Sermon,
    Handwritten,
    Slide,
    Page,
    Note,
}

/// Where an item is in its life: a note is always `ready`; captures move through the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ItemStatus {
    Recording,
    Uploading,
    Waiting,
    Processing,
    Review,
    Ready,
    Error,
}

/// Who said an anchor's verses are what the item is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AnchorOrigin {
    /// The person: the verse a note is attached to, or references confirmed on review.
    User,
    /// `reference::find_all` over text typed in the apps.
    Typed,
    /// A model, from a photo or the digest step.
    Model,
}

/// A range of verses an item is anchored to, `from` ≤ `to` on the canonical grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Anchor {
    pub from: VerseId,
    pub to: VerseId,
    pub origin: AnchorOrigin,
    /// Where in a recording the passage comes up, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<u64>,
    /// Which page of a photographed item it is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
}

/// What a file is to the item or chat that owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum FileRole {
    Audio,
    AudioPart,
    UploadPart,
    Photo,
    PhotoMarked,
    Thumbnail,
    Text,
    Chat,
    Export,
}

/// A file attached to a record: its bytes are downloaded from `url`, a permanent link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    pub uid: String,
    pub role: FileRole,
    pub content_type: String,
    pub bytes: u64,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// A recording's details, as `recording::Details` holds them, with when it ran (server ms).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    /// The church, ministry or channel it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
}

/// A library item: a note, a sermon or a photographed page, with the verses it is anchored to and
/// its files. A verse can have several.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// The record's id, a UUID the client makes.
    pub uid: String,
    /// The per-user sequence number of the write that last changed this record.
    pub seq: u64,
    /// When the record was deleted (a tombstone other devices drop), in server milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<u64>,
    pub kind: ItemKind,
    pub title: String,
    pub status: ItemStatus,
    /// A note's text, inline because it is edited offline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A sermon's or photo's text, as a `text` file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_file_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_uid: Option<String>,
    /// The sermon a slide or page was captured during, and where in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sermon_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ItemDetails>,
    /// The note this one is a conflict copy of: an edit made against a copy another device had
    /// already changed, kept beside it rather than lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_of: Option<String>,
    /// When it was made or captured, in server milliseconds.
    pub captured_at: u64,
    /// When the record was last written, in server milliseconds, stamped by the server on every
    /// write. Absent only on records written before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    /// Whether a model wrote parts the person hasn't reviewed.
    pub unreviewed: bool,
    pub anchors: Vec<Anchor>,
    pub files: Vec<FileRef>,
}

/// A saved Ask conversation; the conversation itself is a `chat` file, downloaded from `url`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    /// The record's id, a UUID the client makes.
    pub uid: String,
    /// The per-user sequence number of the write that last changed this record.
    pub seq: u64,
    /// When the record was deleted (a tombstone other devices drop), in server milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<u64>,
    /// When the conversation began, in server milliseconds.
    pub created_at: u64,
    /// When the record was last written, in server milliseconds, stamped by the server on every
    /// write. Absent only on records written before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    pub title: String,
    pub file_uid: String,
    pub url: String,
    pub bytes: u64,
}

/// One record in the change feed, of any synced type, tagged with that type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FeedRecord {
    Highlight(Highlight),
    Bookmark(Bookmark),
    Item(Box<Item>),
    Chat(Chat),
}

impl FeedRecord {
    pub fn seq(&self) -> u64 {
        match self {
            FeedRecord::Highlight(r) => r.seq,
            FeedRecord::Bookmark(r) => r.seq,
            FeedRecord::Item(r) => r.seq,
            FeedRecord::Chat(r) => r.seq,
        }
    }
}

/// What `library/sync.changes({ after })` answers: the next page of the feed, or `resync` when
/// `after` is older than the tombstones the server still keeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum Changes {
    Page {
        /// Every record with `seq > after`, tombstones included, in `seq` order.
        records: Vec<FeedRecord>,
        /// The highest `seq` in `records`, or `after` when there are none: the next cursor.
        seq: u64,
        /// The server's clock when the page was read, in milliseconds.
        server_time: u64,
    },
    Resync {
        /// Always `true`: empty the cache and pull the library again from 0.
        resync: bool,
    },
}

/// One record type's JSON Schema, as the generators read it.
pub struct RecordSchema {
    pub name: &'static str,
    pub schema: Value,
}

/// Every synced record, in the order they're published.
pub fn schemas() -> Vec<RecordSchema> {
    let mut generator = SchemaSettings::draft2020_12().with(|s| s.inline_subschemas = true).into_generator();
    vec![
        RecordSchema {
            name: "Highlight",
            schema: sorted(generator.root_schema_for::<Highlight>().to_value()),
        },
        RecordSchema { name: "Bookmark", schema: sorted(generator.root_schema_for::<Bookmark>().to_value()) },
        RecordSchema { name: "Item", schema: sorted(generator.root_schema_for::<Item>().to_value()) },
        RecordSchema { name: "Chat", schema: sorted(generator.root_schema_for::<Chat>().to_value()) },
        RecordSchema {
            name: "FeedRecord",
            schema: sorted(generator.root_schema_for::<FeedRecord>().to_value()),
        },
        RecordSchema { name: "Changes", schema: sorted(generator.root_schema_for::<Changes>().to_value()) },
    ]
}

const TS_HEADER: &str = "\
/**
 * GENERATED by `cargo xtask schema` from crates/core/src/records.rs. Do not edit.
 *
 * Every synced record, as an Effect `Schema` (what handlers decode and the proxy encodes) and as
 * Convex validator fields (what convex/schema.ts composes with its indexes). A test in linger-core
 * fails when this file is out of date.
 */
import { Schema } from \"effect\"
import { v } from \"convex/values\"

";

/// The checked-in `server/shared/records.ts`.
pub fn records_ts() -> String {
    let mut out = String::from(TS_HEADER);
    for record in schemas() {
        let object = record.schema.as_object().expect("a record is an object");
        if let Some(description) = description(object) {
            writeln!(out, "/** {description} */").unwrap();
        }
        if object.contains_key("properties") {
            let (effect, convex) = fields(object);
            writeln!(out, "export const {}Fields = {{\n{convex}}}\n", record.name).unwrap();
            writeln!(out, "export const {0} = Schema.Struct({{\n{effect}}})\n", record.name).unwrap();
        } else {
            // A union (a tagged enum or an untagged one): one schema and one validator.
            let (effect, convex) = render(object);
            writeln!(out, "export const {}Validator = {convex}\n", record.name).unwrap();
            writeln!(out, "export const {} = {effect}\n", record.name).unwrap();
        }
        writeln!(out, "export type {0} = typeof {0}[\"Type\"]\n", record.name).unwrap();
    }
    out
}

/// A doc comment as one line, for a `/** */` comment in the generated file.
fn description(object: &Map<String, Value>) -> Option<String> {
    let text = object.get("description").and_then(Value::as_str)?;
    Some(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// An object's properties as Effect struct fields and as Convex validator fields, both indented
/// for a top-level declaration.
fn fields(object: &Map<String, Value>) -> (String, String) {
    let properties = object.get("properties").and_then(Value::as_object).expect("a record has properties");
    let required: Vec<&str> = object
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut effect = String::new();
    let mut convex = String::new();
    for (name, schema) in properties {
        let property = schema.as_object().expect("a property is an object");
        let optional = !required.contains(&name.as_str());
        // An optional field is absent, never null (records.rs skips `None` when serializing), so
        // the `null` serde adds to an `Option`'s schema is dropped.
        let (mut e, mut c) = if optional { render(&without_null(property)) } else { render(property) };
        if optional {
            e = format!("Schema.optionalKey({e})");
            c = format!("v.optional({c})");
        }
        for out in [&mut effect, &mut convex] {
            if let Some(d) = description(property) {
                writeln!(out, "  /** {d} */").unwrap();
            }
        }
        writeln!(effect, "  {name}: {e},").unwrap();
        writeln!(convex, "  {name}: {c},").unwrap();
    }
    (effect, convex)
}

/// One schema as an Effect schema expression and a Convex validator expression.
fn render(schema: &Map<String, Value>) -> (String, String) {
    assert!(
        !schema.contains_key("$ref"),
        "subschemas are inlined; a $ref means the generator settings changed"
    );
    if let Some(literals) = schema.get("enum").and_then(Value::as_array) {
        let quoted: Vec<String> = literals.iter().map(|l| serde_json::to_string(l).unwrap()).collect();
        return if let [one] = quoted.as_slice() {
            (format!("Schema.Literal({one})"), format!("v.literal({one})"))
        } else {
            let convex: Vec<String> = quoted.iter().map(|l| format!("v.literal({l})")).collect();
            (format!("Schema.Literals([{}])", quoted.join(", ")), format!("v.union({})", convex.join(", ")))
        };
    }
    if let Some(literal) = schema.get("const") {
        let quoted = serde_json::to_string(literal).unwrap();
        return (format!("Schema.Literal({quoted})"), format!("v.literal({quoted})"));
    }
    if let Some(members) = schema.get("anyOf").or_else(|| schema.get("oneOf")).and_then(Value::as_array) {
        let (nulls, rest): (Vec<&Value>, Vec<&Value>) = members.iter().partition(|m| m["type"] == "null");
        let rendered: Vec<(String, String)> = rest.iter().map(|m| render(m.as_object().unwrap())).collect();
        let (e, c) = if let [one] = rendered.as_slice() {
            one.clone()
        } else {
            let es: Vec<&str> = rendered.iter().map(|r| r.0.as_str()).collect();
            let cs: Vec<&str> = rendered.iter().map(|r| r.1.as_str()).collect();
            (format!("Schema.Union([{}])", es.join(", ")), format!("v.union({})", cs.join(", ")))
        };
        return if nulls.is_empty() { (e, c) } else { nullable(e, c) };
    }
    let (kind, nullable_type) = match schema.get("type") {
        Some(Value::String(s)) => (s.as_str(), false),
        Some(Value::Array(kinds)) => {
            let non_null: Vec<&str> =
                kinds.iter().filter_map(Value::as_str).filter(|k| *k != "null").collect();
            let [one] = non_null.as_slice() else { panic!("a union of types {kinds:?} isn't supported") };
            (*one, non_null.len() < kinds.len())
        }
        other => panic!("a schema without a type: {other:?}"),
    };
    let (e, c) = match kind {
        "string" => ("Schema.String".to_string(), "v.string()".to_string()),
        "integer" => ("Schema.Int".to_string(), "v.number()".to_string()),
        "number" => ("Schema.Number".to_string(), "v.number()".to_string()),
        "boolean" => ("Schema.Boolean".to_string(), "v.boolean()".to_string()),
        "array" => {
            let items = schema.get("items").and_then(Value::as_object).expect("an array has items");
            let (e, c) = render(items);
            (format!("Schema.Array({e})"), format!("v.array({c})"))
        }
        "object" => {
            let (e, c) = fields(schema);
            let indent = |s: &str| s.lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n");
            (
                format!("Schema.Struct({{\n{}\n  }})", indent(&e)),
                format!("v.object({{\n{}\n  }})", indent(&c)),
            )
        }
        other => panic!("unsupported schema type {other}"),
    };
    if nullable_type { nullable(e, c) } else { (e, c) }
}

/// The schema with `null` removed from its `type` list or `anyOf`.
fn without_null(schema: &Map<String, Value>) -> Map<String, Value> {
    let mut out = schema.clone();
    if let Some(Value::Array(kinds)) = out.get("type").cloned() {
        let rest: Vec<Value> = kinds.into_iter().filter(|k| k != "null").collect();
        out.insert("type".into(), if let [one] = rest.as_slice() { one.clone() } else { Value::Array(rest) });
    }
    if let Some(Value::Array(members)) = out.get("anyOf").cloned() {
        let rest: Vec<Value> = members.into_iter().filter(|m| m["type"] != "null").collect();
        if let [one] = rest.as_slice() {
            out.remove("anyOf");
            for (k, v) in one.as_object().unwrap() {
                out.insert(k.clone(), v.clone());
            }
        } else {
            out.insert("anyOf".into(), Value::Array(rest));
        }
    }
    out
}

fn nullable(e: String, c: String) -> (String, String) {
    (format!("Schema.NullOr({e})"), format!("v.union({c}, v.null())"))
}

/// The checked-in `server/convex/lib/canon.json`: for each book in canon order, its OSIS name,
/// display name and the verses in each chapter.
pub fn canon_json() -> String {
    let mut out = String::from("{\n  \"books\": [\n");
    for (i, book) in BOOKS.iter().enumerate() {
        let id = BookId::new(i).unwrap();
        let verses: Vec<String> =
            (1..=book.chapters).map(|c| ChapterIdx::new(id, c).unwrap().kjv_verses().to_string()).collect();
        let comma = if i + 1 < BOOKS.len() { "," } else { "" };
        writeln!(
            out,
            "    {{ \"osis\": {}, \"name\": {}, \"verses\": [{}] }}{comma}",
            json!(book.osis),
            json!(book.name),
            verses.join(", ")
        )
        .unwrap();
    }
    out.push_str("  ]\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked_in(relative: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../server").join(relative);
        std::fs::read_to_string(&path).unwrap_or_default()
    }

    #[test]
    fn the_checked_in_records_are_current() {
        assert!(
            checked_in("shared/records.ts") == records_ts(),
            "server/shared/records.ts is out of date with crates/core/src/records.rs; run `cargo xtask schema`"
        );
    }

    #[test]
    fn the_checked_in_canon_is_current() {
        assert!(
            checked_in("convex/lib/canon.json") == canon_json(),
            "server/convex/lib/canon.json is out of date with the grid in books.rs; run `cargo xtask schema`"
        );
    }

    #[test]
    fn verse_ids_are_the_documented_integer() {
        let john = BookId::new(42).unwrap();
        assert_eq!(john.name(), "John");
        let id = VerseId::new(john, 3, 16);
        assert_eq!(id, VerseId(42_003_016));
        assert_eq!(id.verse_ref(), Some(VerseRef::new(john, 3, 16)));
        assert_eq!(id.to_string(), "John 3:16");
        assert_eq!(VerseId::from(VerseRef::new(john, 3, 16)), id);
        assert_eq!(VerseId(0).verse_ref(), None, "verse 0 of chapter 0");
        assert_eq!(VerseId(1_001_001).verse_ref().unwrap().to_string(), "Exodus 1:1");
        assert_eq!(VerseId(42_022_001).verse_ref(), None, "John has 21 chapters");
        assert_eq!(VerseId(42_003_037).verse_ref(), None, "John 3 has 36 verses");
        assert_eq!(VerseId(66_001_001).verse_ref(), None, "there is no 67th book");
        assert!(VerseId(1_001).is_valid(), "Genesis 1:1");
        assert_eq!(VerseId(99_999_999).to_string(), "#99999999");
    }

    #[test]
    fn records_serialize_camel_cased_without_absent_fields() {
        let h = Highlight {
            uid: "h-42003016".into(),
            seq: 7,
            deleted_at: None,
            created_at: None,
            verse: VerseId(42_003_016),
            color: HighlightColor::Yellow,
        };
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v, json!({ "uid": "h-42003016", "seq": 7, "verse": 42_003_016, "color": "yellow" }));
        let back: Highlight = serde_json::from_value(json!({
            "uid": "h-42003016", "seq": 8, "deletedAt": 1_700_000_000_000u64, "createdAt": 1_600_000_000_000u64,
            "verse": 42_003_016, "color": "blue"
        }))
        .unwrap();
        assert_eq!(back.deleted_at, Some(1_700_000_000_000));
        assert_eq!(back.created_at, Some(1_600_000_000_000));
        assert_eq!(back.color, HighlightColor::Blue);
    }

    #[test]
    fn every_schema_is_inlined_and_every_record_is_an_object() {
        for record in schemas() {
            let text = record.schema.to_string();
            assert!(!text.contains("$ref"), "{} has a $ref: {text}", record.name);
            if record.schema.get("properties").is_none() {
                assert!(
                    record.schema.get("oneOf").or(record.schema.get("anyOf")).is_some(),
                    "{}",
                    record.name
                );
                continue;
            }
            assert_eq!(record.schema["type"], "object", "{}", record.name);
            let required = record.schema["required"].as_array().unwrap();
            assert!(required.iter().any(|r| r == "uid") && required.iter().any(|r| r == "seq"));
            assert!(!required.iter().any(|r| r == "deletedAt"), "tombstone time is optional");
        }
    }

    #[test]
    fn the_generated_typescript_names_every_record_both_ways() {
        let ts = records_ts();
        for record in schemas() {
            if record.schema.get("properties").is_some() {
                assert!(ts.contains(&format!("export const {}Fields = {{", record.name)));
                assert!(ts.contains(&format!("export const {} = Schema.Struct({{", record.name)));
            } else {
                assert!(ts.contains(&format!("export const {}Validator = v.union(", record.name)));
                assert!(ts.contains(&format!("export const {} = Schema.Union([", record.name)));
            }
            assert!(ts.contains(&format!("export type {0} = typeof {0}[\"Type\"]", record.name)));
        }
        assert!(ts.contains("deletedAt: Schema.optionalKey(Schema.Int),"));
        assert!(ts.contains("deletedAt: v.optional(v.number()),"));
        assert!(ts.contains(
            "color: Schema.Literals([\"yellow\", \"green\", \"blue\", \"pink\", \"purple\", \"orange\"]),"
        ));
        assert!(ts.contains("color: v.union(v.literal(\"yellow\"), v.literal(\"green\"),"));
        assert!(ts.contains("type: Schema.Literal(\"highlight\"),"));
        assert!(ts.contains("type: v.literal(\"bookmark\"),"));
    }

    #[test]
    fn the_feed_reads_what_sync_changes_sends() {
        let page: Changes = serde_json::from_value(json!({
            "records": [
                { "type": "highlight", "uid": "h-42003016", "seq": 4, "verse": 42_003_016, "color": "pink" },
                { "type": "bookmark", "uid": "b-1001001", "seq": 5, "deletedAt": 9, "verse": 1_001_001, "label": "" }
            ],
            "seq": 5,
            "serverTime": 1_800_000_000_000u64
        }))
        .unwrap();
        let Changes::Page { records, seq, server_time } = page else { panic!("a page") };
        assert_eq!((seq, server_time), (5, 1_800_000_000_000));
        assert_eq!(records.iter().map(FeedRecord::seq).collect::<Vec<_>>(), [4, 5]);
        assert!(
            matches!(&records[1], FeedRecord::Bookmark(b) if b.deleted_at == Some(9) && b.verse == VerseId(1_001_001))
        );
        assert_eq!(
            serde_json::from_value::<Changes>(json!({ "resync": true })).unwrap(),
            Changes::Resync { resync: true }
        );
        // An empty page is a page, not a resync.
        let empty: Changes =
            serde_json::from_value(json!({ "records": [], "seq": 0, "serverTime": 1 })).unwrap();
        assert!(matches!(empty, Changes::Page { .. }));
    }

    #[test]
    fn the_canon_covers_the_whole_grid() {
        let canon: Value = serde_json::from_str(&canon_json()).unwrap();
        let books = canon["books"].as_array().unwrap();
        assert_eq!(books.len(), 66);
        let chapters: usize = books.iter().map(|b| b["verses"].as_array().unwrap().len()).sum();
        assert_eq!(chapters, 1189);
        let verses: u64 =
            books.iter().flat_map(|b| b["verses"].as_array().unwrap()).map(|v| v.as_u64().unwrap()).sum();
        assert_eq!(verses, 31_102);
        assert_eq!(books[42]["osis"], "John");
        assert_eq!(books[42]["verses"][2], 36);
    }
}
