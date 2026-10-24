//! Conversations with the assistant: messages, the citations inside them, and the tools the
//! assistant runs on the device.
//!
//! The model never writes Scripture from memory. It cites references as `{{John 3:16}}` (or a
//! range, `{{John 3:16-18}}`); the app turns each into a chip that opens the verse in the reader's
//! own translation. Any verse text the model reads comes through [`Tools`], from a bundled
//! public-domain translation ([`ask_translation`]): copyrighted text must never reach a model,
//! whatever translation the reader shows. The same tools serve the model's tool calls and the
//! offline stand-in, [`local_reply`].
//!
//! The user's library (their sermons and notes) is cited the same way, by item and place:
//! `{{item <uid> 12:30}}` for a moment in a sermon, `{{item <uid> p3}}` for a page, `{{item <uid>}}`
//! for a note. Chats from before the library cite sermons as `{{sermon 1a2b3c4d 12:30}}`, which
//! still parses. The library tools reach Convex through one door, [`UserLibrary`]: on a device
//! the sync thread's connection, so there is no second client.

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::books::{BookId, ChapterIdx, VerseRef};
use crate::crossrefs::CrossRefs;
use crate::model::Translation;
use crate::names::{Category, Entity, Names};
use crate::original::{self, Original};
use crate::records::{ItemText, SearchHit, VerseId};
use crate::reference::{self, Reference};
use crate::search::{self, Scope};
use crate::transcribe;

/// The translations Ask may read: bundled and public domain, so their text may go to a model.
/// The first is the default.
pub const ASK_TRANSLATIONS: [&str; 3] = ["bsb", "web", "kjv"];

/// The translation Ask's tools read, from the user's preference: one of [`ASK_TRANSLATIONS`],
/// and the BSB for anything else (a licensed or imported translation is never used).
pub fn ask_translation(preference: &str) -> &'static str {
    ASK_TRANSLATIONS.iter().copied().find(|id| *id == preference).unwrap_or(ASK_TRANSLATIONS[0])
}

/// For the model, with every question: "Passages you read and search come from the BSB (Berean
/// Standard Bible). The user's library has 12 items; 3 of them touch John 3. Their own church is
/// Grace Church." The system prompt says what to do with the library; this says how much there is,
/// how much of it bears on the chapter being read, and which church is the user's, so a sermon
/// from elsewhere isn't taken for it. Nothing is said of the library when there is none to reach.
pub fn tools_note(tools: &Tools, chapter: Option<ChapterIdx>) -> String {
    let t = &tools.translation;
    let mut note = format!("Passages you read and search come from the {} ({}).", t.meta.abbrev, t.meta.name);
    if let Some(library) = &tools.library
        && let Ok((total, here)) = library.counts(chapter)
    {
        let items = format!("{total} item{}", if total == 1 { "" } else { "s" });
        match (total, here, chapter) {
            (0, ..) => note.push_str(" The user's library is empty."),
            (_, 0, _) | (_, _, None) => note.push_str(&format!(" The user's library has {items}.")),
            (_, 1, Some(c)) => {
                note.push_str(&format!(" The user's library has {items}; 1 of them touches {}.", c.display()))
            }
            (_, n, Some(c)) => {
                note.push_str(&format!(" The user's library has {items}; {n} of them touch {}.", c.display()))
            }
        }
    }
    if let Some(church) = &tools.church {
        note.push_str(&format!(" Their own church is {church}."));
    }
    note
}

/// What the user is looking at when they ask: sent with every question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub chapter: ChapterIdx,
    /// An inclusive verse range.
    pub selection: Option<(VerseRef, VerseRef)>,
    /// The abbreviation of the translation the reader shows ("NLT"): only its name goes to the
    /// model, never its text.
    pub reading: String,
    /// The library item being asked about, from its page: its uid and title.
    pub item: Option<(String, String)>,
}

impl Context {
    /// For the model: "The user is reading John 3 in the NLT. They have selected John 3:14."
    pub fn describe(&self) -> String {
        let mut s = format!("The user is reading {} in the {}.", self.chapter.display(), self.reading);
        if let Some((a, b)) = self.selection {
            s.push_str(&format!(" They have selected {}.", reference::format_range(a, b)));
        }
        if let Some((uid, title)) = &self.item {
            s.push_str(&format!(" They are asking about “{title}”, item {uid} in their library."));
        }
        s
    }
}

/// A conversation's title in Recents: its first question's first line, cut at a word to about
/// 60 characters.
pub fn title_for(question: &str) -> String {
    let line = question.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("New conversation");
    if line.chars().count() <= 60 {
        return line.to_string();
    }
    let cut: String = line.chars().take(60).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ';', ':', '.', ' ']))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    /// Plain text with `{{reference}}` citations.
    pub text: String,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: Role::User, text: text.into() }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self { role: Role::Assistant, text: text.into() }
    }

    /// The verse ranges the message cites, in order, without repeats.
    pub fn cited(&self) -> Vec<(VerseRef, VerseRef)> {
        let mut out = Vec::new();
        for seg in segments(&self.text) {
            if let Segment::Verse(r) = seg
                && let Some(range) = verse_range(&r)
                && !out.contains(&range)
            {
                out.push(range);
            }
        }
        out
    }
}

/// A piece of a message's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Verse(Reference),
    /// A place in a library item: its uid (or its first characters), and for a sermon the moment,
    /// for a photographed item the page; neither for a note.
    Item {
        uid: String,
        at_ms: Option<u64>,
        page: Option<u32>,
    },
    /// A moment in a recorded sermon, as chats from before the library cite it: the recording id
    /// (or its first characters) and where.
    Sermon {
        id: String,
        at_ms: u64,
    },
}

/// Splits text at `{{reference}}` citations. A marker that isn't a reference stays as text.
pub fn segments(text: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut pending = String::new();
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start + 2..].find("}}") else { break };
        let inner = &rest[start + 2..start + 2 + len];
        pending.push_str(&rest[..start]);
        match parse_citation(inner).map(Segment::Verse).or_else(|| parse_item_citation(inner)) {
            Some(seg) => {
                if !pending.is_empty() {
                    out.push(Segment::Text(std::mem::take(&mut pending)));
                }
                out.push(seg);
            }
            None => pending.push_str(&rest[start..start + 2 + len + 2]),
        }
        rest = &rest[start + 2 + len + 2..];
    }
    pending.push_str(rest);
    if !pending.is_empty() {
        out.push(Segment::Text(pending));
    }
    out
}

/// A citation must name its book exactly (by name, abbreviation or a known alias): the
/// typo-tolerant parsing is for what people type, not for what the model writes.
fn parse_citation(inner: &str) -> Option<Reference> {
    let r = reference::parse(inner.trim())?;
    (exact_book(&book_key(inner)) == Some(r.book)).then_some(r)
}

/// `item <uid> 12:30`, `item <uid> p3` or `item <uid>`; and the older `sermon 1a2b3c4d 12:30`.
fn parse_item_citation(inner: &str) -> Option<Segment> {
    let mut words = inner.split_whitespace();
    let kind = words.next()?;
    let id = words.next()?.to_string();
    if id.len() < 4 || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    let place = words.next();
    if words.next().is_some() {
        return None;
    }
    if kind.eq_ignore_ascii_case("sermon") {
        return Some(Segment::Sermon { id, at_ms: transcribe::parse_clock(place?)? });
    }
    if !kind.eq_ignore_ascii_case("item") {
        return None;
    }
    let (at_ms, page) = match place {
        None => (None, None),
        Some(p) => match p.strip_prefix(['p', 'P']).and_then(|n| n.parse::<u32>().ok()) {
            Some(n) if n >= 1 => (None, Some(n)),
            _ => (Some(transcribe::parse_clock(p)?), None),
        },
    };
    Some(Segment::Item { uid: id, at_ms, page })
}

fn exact_book(key: &str) -> Option<BookId> {
    let squash = |s: &str| s.to_lowercase().replace([' ', '.'], "");
    BookId::all().find(|b| {
        let i = b.info();
        [i.name, i.abbrev, i.code, i.osis].iter().any(|n| squash(n) == key) || i.aliases.contains(&key)
    })
}

/// The book part of a reference, squashed: "1 Cor 13:4" → "1cor", "Song of Solomon 2" →
/// "songofsolomon".
fn book_key(s: &str) -> String {
    let s = s.trim();
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    let (ordinal, rest) = s.split_at(digits);
    let name: String = rest.chars().take_while(|c| !c.is_ascii_digit()).collect();
    format!("{ordinal}{}", name.to_lowercase().replace([' ', '.'], ""))
}

/// The verses a reference covers; a whole chapter for a chapter reference.
pub fn verse_range(r: &Reference) -> Option<(VerseRef, VerseRef)> {
    let c = ChapterIdx::new(r.book, r.chapter)?;
    Some(match (r.verse, r.verse_end) {
        (None, _) => (VerseRef::new(r.book, r.chapter, 1), VerseRef::new(r.book, r.chapter, c.kjv_verses())),
        (Some(a), None) => (VerseRef::new(r.book, r.chapter, a), VerseRef::new(r.book, r.chapter, a)),
        (Some(a), Some(b)) => {
            (VerseRef::new(r.book, r.chapter, a), VerseRef::new(r.book, r.chapter, b.max(a)))
        }
    })
}

/// The tools the assistant can call, run against a public-domain translation (see
/// [`ask_translation`]) and the user's library.
pub struct Tools {
    pub translation: Arc<Translation>,
    /// The user's library, when there is one to reach: `None` signed out, on the web, and when
    /// the user asked for an answer from the Bible only.
    pub library: Option<Arc<dyn UserLibrary>>,
    /// The user's own church, as they named it in Settings.
    pub church: Option<String>,
}

/// The user's library, as the Ask tools reach it: the one door to Convex from a device
/// ([`sync::SyncHandle::user_library`](crate::sync::SyncHandle::user_library), the sync thread's
/// own connection), or a fake in tests. Arguments are JSON as the server's functions take them;
/// answers are the shapes the server declares.
pub trait UserLibrary: Send + Sync {
    /// `library/search.search`; offline, what the cached items' text finds, and `true`.
    fn search(&self, args: Value) -> Result<(Vec<SearchHit>, bool)>;
    /// `library/items.read`: `None` when there is no such item.
    fn read(&self, args: Value) -> Result<Option<ItemText>>;
    /// How many items the library holds, and how many of them are anchored in `chapter`.
    fn counts(&self, chapter: Option<ChapterIdx>) -> Result<(usize, usize)>;
}

/// The most verses one `get_passage` call returns, so a chapter of Psalm 119 can't flood the
/// conversation.
const PASSAGE_LIMIT: usize = 60;
/// Verses one `original_words` call returns at most: each is a few kilobytes of words.
const ORIGINAL_VERSES: usize = 5;
/// Lexicon entries one `lexicon` call returns in full; more are listed by number and gloss.
const LEXICON_ENTRIES: usize = 6;
/// Characters of one definition: Abbott-Smith's longest entries run to tens of thousands.
const DEFINITION_CHARS: usize = 5000;
/// Verses of a cross-referenced passage quoted in full; a longer one is cut short.
const CROSS_REFERENCE_VERSES: usize = 3;
/// People or places one `people_and_places` call describes in full; more are listed by id.
const NAMES_IN_FULL: usize = 3;
/// People or places one `people_and_places` call lists.
const NAMES_LISTED: usize = 30;
/// Verses an entity is named in that `people_and_places` lists.
const NAMED_VERSES: usize = 12;

impl Tools {
    /// Tool definitions in the Anthropic Messages API's `tools` format.
    pub fn definitions() -> Value {
        json!([
            {
                "name": "search_bible",
                "description": "Full-text search of the Bible, in the translation named with the question. \
                    Every word must appear (as a word prefix); use \"quotes\" for an exact phrase and -word \
                    to exclude. Returns matching verses with their text, in canonical order.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "scope": {
                            "type": "string",
                            "description": "\"all\" (default), \"ot\", \"nt\", or a book name such as \"Romans\"."
                        },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            },
            {
                "name": "search_library",
                "description": "Searches the user's library: what they have heard, read and written in Linger, \
                    which is sermons they recorded (at their own church or from elsewhere, such as a livestream \
                    or a video) and their notes. It is the user's own material: not Scripture, and not \
                    necessarily right; an unreviewed item's text was made by machine and may have reading \
                    mistakes. Every word must appear in a passage (the last as a word prefix; use \"quotes\" \
                    for an exact phrase). Returns passages best first, grouped by item, each with the item's \
                    id, kind, title, who it is from, its date, the moment or page it is at, and whether the \
                    item is anchored to the passage given.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "passage": {
                            "type": "string",
                            "description": "The passage in question (\"Romans 8\", \"John 3:1-21\"): items \
                                anchored to it come first."
                        },
                        "kinds": {
                            "type": "array",
                            "items": { "type": "string", "enum": ["sermon", "note", "handwritten", "slide", "page"] },
                            "description": "Only items of these kinds."
                        },
                        "source": {
                            "type": "string",
                            "description": "A preacher's or church's name: only what is from them."
                        },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            },
            {
                "name": "read_item",
                "description": "Reads an item from the user's library by its id (from search_library or a \
                    citation): a note's text; a sermon's transcript from a moment on (\"12:30\"; the start by \
                    default), up to about 15 minutes, each paragraph stamped [mm:ss]; a photographed item's \
                    text, or one page of it with page.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "item": { "type": "string" },
                        "from": { "type": "string" },
                        "minutes": { "type": "integer", "minimum": 1, "maximum": 15 },
                        "page": { "type": "integer", "minimum": 1 }
                    },
                    "required": ["item"],
                    "additionalProperties": false
                }
            },
            {
                "name": "original_words",
                "description": "The Hebrew (Old Testament) or Greek (New Testament) behind a verse or a few \
                    verses, word by word: each word as written and transliterated, its English sense in this \
                    verse, its dictionary form (lemma) with its Strong's number, gloss and grammar. Up to 5 \
                    verses. Use it before saying what a word means in the original.",
                "input_schema": {
                    "type": "object",
                    "properties": { "reference": { "type": "string" } },
                    "required": ["reference"],
                    "additionalProperties": false
                }
            },
            {
                "name": "lexicon",
                "description": "A Hebrew or Greek word's lexicon entry: its definition (Abbott-Smith for Greek; \
                    an outline of Brown-Driver-Briggs and Strong's for Hebrew), gloss and number of \
                    occurrences. Give a Strong's number from original_words (\"G3056\", \"H1254A\") or a lemma \
                    as original_words gives it; a number without its letter returns every sense.",
                "input_schema": {
                    "type": "object",
                    "properties": { "word": { "type": "string" } },
                    "required": ["word"],
                    "additionalProperties": false
                }
            },
            {
                "name": "word_occurrences",
                "description": "Where a Hebrew or Greek word is used, for a word study: how often in each book, \
                    the English senses it is given and how often, and the verses (with their text in the \
                    translation named with the question) in canonical order. Give a Strong's number or lemma \
                    as for lexicon.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "word": { "type": "string" },
                        "scope": {
                            "type": "string",
                            "description": "\"all\" (default), \"ot\", \"nt\", or a book name such as \"John\"."
                        },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                    },
                    "required": ["word"],
                    "additionalProperties": false
                }
            },
            {
                "name": "cross_references",
                "description": "Other passages that speak to a verse or passage (up to a chapter), strongest \
                    first, with their text in the translation named with the question: OpenBible.info's \
                    cross-references, which grew from the Treasury of Scripture Knowledge, with votes for \
                    how many readers found each link helpful. Use it to let Scripture interpret Scripture \
                    and to find where a passage is quoted or echoed.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "reference": { "type": "string" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 30 }
                    },
                    "required": ["reference"],
                    "additionalProperties": false
                }
            },
            {
                "name": "people_and_places",
                "description": "The Bible's people, places and other proper names (deities, months, titles), \
                    each individual told apart: the many Zechariahs are separate entries. Give a name in any \
                    translation's spelling (\"Elijah\", \"Elias\"), a Strong's number from original_words or an \
                    id from an earlier result (\"Zechariah@2Ch.24.20\": the name and the first verse it is \
                    found in); add a reference to keep only who or what is named there, or give only a \
                    reference to list everyone and every place named in it. Up to 3 matches come in full: \
                    the forms of the name in English and Hebrew or Greek, family and tribe, where it is \
                    named, and for a place the modern locations scholars identify it with, each with their \
                    confidence (0-100) and coordinates. With more, the most often named (or, for a passage, \
                    the first named) comes in full and the others as a list to choose from by id.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "reference": { "type": "string" }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "get_passage",
                "description": "The text of a passage, in the translation named with the question: a verse \
                    (\"John 3:16\"), a range (\"John 3:16-21\") or a chapter (\"Psalm 23\"). Quote \
                    Scripture only from this.",
                "input_schema": {
                    "type": "object",
                    "properties": { "reference": { "type": "string" } },
                    "required": ["reference"],
                    "additionalProperties": false
                }
            }
        ])
    }

    pub fn run(&self, name: &str, input: &Value) -> Result<Value> {
        match name {
            "search_bible" => {
                let query = input["query"].as_str().context("query is required")?;
                let scope = scope(input["scope"].as_str().unwrap_or("all"))?;
                let limit = input["limit"].as_u64().unwrap_or(10).clamp(1, 50) as usize;
                let outcome = search::run(&self.translation, query, scope, false);
                let hits: Vec<Value> = outcome
                    .verses
                    .iter()
                    .take(limit)
                    .filter_map(|&i| self.translation.verse_view(i))
                    .map(|v| json!({ "reference": v.reference().display(), "text": v.plain_text() }))
                    .collect();
                Ok(json!({ "total": outcome.verses.len(), "verses": hits }))
            }
            "get_passage" => {
                let text = input["reference"].as_str().context("reference is required")?;
                // Strict: a forgiving parse would turn a chapter that doesn't exist ("John 22") into
                // another book's ("Job 22") and quote the wrong text.
                let r = parse_citation(text).with_context(|| format!("not a reference: {text:?}"))?;
                let (verses, more) = self.passage(&r);
                if verses.is_empty() {
                    bail!("{} is not in the {}", r.display(), self.translation.meta.abbrev);
                }
                let verses: Vec<Value> =
                    verses.into_iter().map(|(n, t)| json!({ "verse": n, "text": t })).collect();
                Ok(json!({
                    "reference": r.display(),
                    "translation": self.translation.meta.abbrev,
                    "verses": verses,
                    "truncated": more,
                }))
            }
            "search_library" => {
                let library = self.library()?;
                let query = input["query"].as_str().context("query is required")?;
                let mut args = json!({ "query": query });
                if let Some(text) = input["passage"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    let r = parse_citation(text).with_context(|| format!("not a reference: {text:?}"))?;
                    let (a, b) =
                        verse_range(&r).with_context(|| format!("no such passage: {}", r.display()))?;
                    args["passage"] = json!({ "from": VerseId::from(a), "to": VerseId::from(b) });
                }
                if let Some(kinds) = input.get("kinds").filter(|k| k.is_array()) {
                    args["kinds"] = kinds.clone();
                }
                if let Some(source) = input["source"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    args["source"] = json!(source);
                }
                if let Some(limit) = input["limit"].as_u64() {
                    args["limit"] = json!(limit.clamp(1, 50));
                }
                let (hits, offline) = library.search(args)?;
                let mut v =
                    json!({ "total": hits.len(), "hits": hits.iter().map(hit_json).collect::<Vec<_>>() });
                if offline {
                    v["offline"] = json!(
                        "The device is offline: this searched only the items cached on it, by their words."
                    );
                }
                Ok(v)
            }
            "read_item" => {
                let library = self.library()?;
                let item = input["item"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .context("item is required")?;
                let mut args = json!({ "uid": item });
                if let Some(f) = input["from"].as_str() {
                    let from = transcribe::parse_clock(f).with_context(|| format!("not a time: {f:?}"))?;
                    args["fromMs"] = json!(from);
                }
                if let Some(m) = input["minutes"].as_u64() {
                    args["minutes"] = json!(m.clamp(1, 15));
                }
                if let Some(p) = input["page"].as_u64() {
                    args["page"] = json!(p.max(1));
                }
                match library.read(args)? {
                    Some(t) => Ok(item_text_json(&t)),
                    None => bail!("no item {item:?} in the user's library: it may have been deleted"),
                }
            }
            "original_words" => {
                let text = input["reference"].as_str().context("reference is required")?;
                let r = parse_citation(text).with_context(|| format!("not a reference: {text:?}"))?;
                self.original_words(&r)
            }
            "lexicon" => {
                let word = input["word"].as_str().context("word is required")?;
                lexicon(original()?, word)
            }
            "word_occurrences" => {
                let word = input["word"].as_str().context("word is required")?;
                let scope = scope(input["scope"].as_str().unwrap_or("all"))?;
                let limit = input["limit"].as_u64().unwrap_or(15).clamp(1, 50) as usize;
                self.word_occurrences(word, scope, limit)
            }
            "cross_references" => {
                let text = input["reference"].as_str().context("reference is required")?;
                let r = parse_citation(text).with_context(|| format!("not a reference: {text:?}"))?;
                let limit = input["limit"].as_u64().unwrap_or(10).clamp(1, 30) as usize;
                self.cross_references(&r, limit)
            }
            "people_and_places" => {
                let name = input["name"].as_str().map(str::trim).filter(|n| !n.is_empty());
                let passage = match input["reference"].as_str() {
                    Some(text) => {
                        let r = parse_citation(text).with_context(|| format!("not a reference: {text:?}"))?;
                        Some(verse_range(&r).with_context(|| format!("no such passage: {}", r.display()))?)
                    }
                    None => None,
                };
                people_and_places(names()?, name, passage)
            }
            other => bail!("unknown tool {other}"),
        }
    }

    fn original_words(&self, r: &Reference) -> Result<Value> {
        let o = original()?;
        let (a, b) = verse_range(r).with_context(|| format!("no such passage: {}", r.display()))?;
        let last = b.verse.min(a.chapter_idx().kjv_verses());
        let mut verses = Vec::new();
        for n in a.verse..=last {
            let words: Vec<Value> =
                o.words_in(VerseRef::new(a.book, a.chapter, n)).map(|w| word_json(o, &w)).collect();
            if !words.is_empty() {
                verses.push(json!({ "verse": n, "words": words }));
            }
        }
        if verses.is_empty() {
            bail!("no such passage: {}", r.display());
        }
        let more = verses.len() > ORIGINAL_VERSES;
        verses.truncate(ORIGINAL_VERSES);
        let language = match a.book.testament() {
            crate::Testament::Old => "Hebrew (Aramaic where a word's grammar says so)",
            crate::Testament::New => "Greek",
        };
        Ok(json!({ "reference": r.display(), "language": language, "verses": verses, "truncated": more }))
    }

    fn word_occurrences(&self, word: &str, scope: Scope, limit: usize) -> Result<Value> {
        let o = original()?;
        let entries = o.lookup(word);
        if entries.is_empty() {
            bail!("no Hebrew or Greek word {word:?}; use a Strong's number or lemma from original_words");
        }
        let strongs: Vec<&str> = entries.iter().map(|e| e.strong).collect();
        let study = o.study(&strongs, scope);
        // One line per verse, with every form of the word in it.
        let mut verses: Vec<(VerseRef, Vec<String>, Vec<String>)> = Vec::new();
        for w in &study.words {
            let english = original::rendering(w, &strongs);
            match verses.last_mut() {
                Some((v, forms, senses)) if *v == w.verse => {
                    forms.push(w.text.to_string());
                    senses.push(english);
                }
                _ => verses.push((w.verse, vec![w.text.to_string()], vec![english])),
            }
        }
        let listed: Vec<Value> = verses
            .iter()
            .take(limit)
            .map(|(v, forms, senses)| {
                let text = self.translation.find(*v).map(|t| t.plain_text()).unwrap_or_default();
                json!({ "reference": v.display(), "original": forms.join(" … "), "english": senses.join(" … "), "text": text })
            })
            .collect();
        let lemmas: Vec<String> =
            entries.iter().map(|e| format!("{} {} ({})", e.strong, e.lemma, e.translit)).collect();
        Ok(json!({
            "word": lemmas.join("; "),
            "total": study.total,
            "verses_with_it": verses.len(),
            "by_book": study.by_book.iter().map(|(b, n)| json!({ "book": b.name(), "count": n })).collect::<Vec<_>>(),
            "renderings": study.renderings.iter().take(12).map(|(e, n)| json!({ "english": e, "count": n })).collect::<Vec<_>>(),
            "verses": listed,
            "truncated": verses.len() > limit,
        }))
    }

    /// The passages `r` links to, strongest first. A passage's verses can point to the same place:
    /// it is listed once, with its strongest link.
    fn cross_references(&self, r: &Reference, limit: usize) -> Result<Value> {
        let x = crossrefs()?;
        let (a, b) = verse_range(r).with_context(|| format!("no such passage: {}", r.display()))?;
        let mut links: Vec<crate::crossrefs::Link> = Vec::new();
        for l in x.from_range(a, b) {
            // A link back into the passage itself says nothing new.
            if l.to <= b && l.to_end >= a {
                continue;
            }
            match links.iter_mut().find(|m| m.to == l.to && m.to_end == l.to_end) {
                Some(m) if m.votes >= l.votes => {}
                Some(m) => *m = *l,
                None => links.push(*l),
            }
        }
        links.sort_by(|p, q| q.votes.cmp(&p.votes).then(p.to.cmp(&q.to)));
        let listed: Vec<Value> = links
            .iter()
            .take(limit)
            .map(|l| {
                let last = if l.to_end.chapter == l.to.chapter { l.to_end.verse } else { u8::MAX };
                let verses: Vec<String> = self
                    .translation
                    .verses_in(l.to.chapter_idx())
                    .filter(|v| v.number_end >= l.to.verse && v.number <= last)
                    .map(|v| v.plain_text())
                    .collect();
                // A passage across chapters is quoted from its first chapter.
                let cut = verses.len() > CROSS_REFERENCE_VERSES || l.to_end.chapter != l.to.chapter;
                let mut v = json!({
                    "reference": crate::reference::format_range(l.to, l.to_end),
                    "votes": l.votes,
                    "text": verses.iter().take(CROSS_REFERENCE_VERSES).cloned().collect::<Vec<_>>().join(" "),
                });
                if a != b {
                    v["from"] = json!(l.from.display());
                }
                if cut {
                    v["text_truncated"] = json!(true);
                }
                v
            })
            .collect();
        Ok(json!({
            "reference": r.display(),
            "total": links.len(),
            "cross_references": listed,
            "truncated": links.len() > limit,
        }))
    }

    /// The library, when there is one to reach.
    fn library(&self) -> Result<&dyn UserLibrary> {
        self.library.as_deref().context(
            "the user's library isn't open to this question: they are signed out, or asked for an answer \
             from the Bible only",
        )
    }

    /// The verses of `r` as (number, text), and whether [`PASSAGE_LIMIT`] cut it short.
    pub fn passage(&self, r: &Reference) -> (Vec<(u8, String)>, bool) {
        let Some((a, b)) = verse_range(r) else { return (Vec::new(), false) };
        let c = a.chapter_idx();
        let mut out: Vec<(u8, String)> = self
            .translation
            .verses_in(c)
            .filter(|v| v.number_end >= a.verse && v.number <= b.verse)
            .map(|v| (v.number, v.plain_text()))
            .collect();
        let more = out.len() > PASSAGE_LIMIT;
        out.truncate(PASSAGE_LIMIT);
        (out, more)
    }
}

/// The Hebrew and Greek text, decoded the first time a tool needs it.
fn original() -> Result<&'static Original> {
    Original::bundled()
}

/// The cross-references, decoded the first time a tool needs them.
fn crossrefs() -> Result<&'static CrossRefs> {
    CrossRefs::bundled()
}

/// The people and places, decoded the first time a tool needs them.
fn names() -> Result<&'static Names> {
    Names::bundled()
}

/// Who or what `name` names, within `passage`; everyone and everything named in `passage`
/// without one.
fn people_and_places(n: &Names, name: Option<&str>, passage: Option<(VerseRef, VerseRef)>) -> Result<Value> {
    let in_passage = |e: &Entity| -> Vec<VerseRef> {
        let Some((a, b)) = passage else { return Vec::new() };
        e.verses().into_iter().filter(|v| (a..=b).contains(v)).collect()
    };
    let found: Vec<&Entity> = match (name, passage) {
        (Some(name), _) => {
            let all = n.lookup(name);
            if all.is_empty() {
                bail!(
                    "no person, place or name {name:?}; try another spelling, or a Strong's number from original_words"
                );
            }
            all.into_iter().filter(|e| passage.is_none() || !in_passage(e).is_empty()).collect()
        }
        (None, Some((a, b))) => n.named_in(a, b),
        (None, None) => bail!("give a name, a reference, or both"),
    };
    let with_passage = |mut v: Value, e: &Entity| {
        if passage.is_some() {
            v["in_passage"] = json!(in_passage(e).iter().map(|r| r.display()).collect::<Vec<_>>());
        }
        v
    };
    // A few in full; with more, the first in full and the rest to choose from.
    let full = if found.len() <= NAMES_IN_FULL { found.len() } else { 1 };
    let matches: Vec<Value> = found[..full].iter().map(|e| with_passage(entity_json(e), e)).collect();
    let mut out = json!({ "total": found.len(), "matches": matches });
    if found.len() > full {
        let others: Vec<Value> =
            found[full..].iter().take(NAMES_LISTED).map(|e| with_passage(entity_summary(e), e)).collect();
        out["others"] = json!(others);
        out["truncated"] = json!(found.len() > full + NAMES_LISTED);
    }
    Ok(out)
}

fn category(e: &Entity) -> &'static str {
    match e.category {
        Category::Person => "person",
        Category::Place => "place",
        Category::Other => "other",
    }
}

/// One line for choosing between people or places of the same name.
fn entity_summary(e: &Entity) -> Value {
    let verses = e.verses();
    let mut v = json!({
        "id": e.id,
        "category": category(e),
        "kind": e.kind,
        "first_named": verses.first().map(|r| r.display()),
        "verses_named_in": verses.len(),
    });
    let tribe = if e.category == Category::Place { "region" } else { "tribe" };
    for (k, s) in [("description", &e.description), (tribe, &e.tribe)] {
        if !s.is_empty() {
            v[k] = json!(s);
        }
    }
    v
}

fn entity_json(e: &Entity) -> Value {
    let mut v = entity_summary(e);
    if let Some(f) = &e.father {
        v["father"] = json!(f);
    }
    if let Some(m) = &e.mother {
        v["mother"] = json!(m);
    }
    for (k, list) in [
        ("siblings", &e.siblings),
        ("partners", &e.partners),
        ("children", &e.offspring),
        ("founded_by", &e.founders),
        ("inhabitants", &e.inhabitants),
    ] {
        if !list.is_empty() {
            v[k] = json!(list);
        }
    }
    v["names"] = json!(
        e.names
            .iter()
            .map(|f| {
                let mut n =
                    json!({ "english": f.english.join(" / "), "strong": f.strong, "verses": f.verses.len() });
                if !f.original.is_empty() {
                    n["original"] = json!(f.original);
                }
                n
            })
            .collect::<Vec<_>>()
    );
    if !e.locations.is_empty() {
        v["locations"] = json!(
            e.locations
                .iter()
                .map(|l| {
                    let mut x = json!({ "identification": l.identification, "confidence": l.confidence });
                    if !l.kind.is_empty() {
                        x["kind"] = json!(l.kind);
                    }
                    if let Some((lat, lon)) = l.lat_lon {
                        x["lat"] = json!(lat);
                        x["lon"] = json!(lon);
                    }
                    x
                })
                .collect::<Vec<_>>()
        );
    }
    let verses = e.verses();
    let mut by_book: Vec<(BookId, usize)> = Vec::new();
    for r in &verses {
        match by_book.last_mut() {
            Some((b, n)) if *b == r.book => *n += 1,
            _ => by_book.push((r.book, 1)),
        }
    }
    v["by_book"] =
        json!(by_book.iter().map(|(b, n)| json!({ "book": b.name(), "count": n })).collect::<Vec<_>>());
    v["first_verses"] = json!(verses.iter().take(NAMED_VERSES).map(|r| r.display()).collect::<Vec<_>>());
    v
}

fn word_json(o: &Original, w: &original::Word<'_>) -> Value {
    let roots = w.roots();
    let entries: Vec<original::Entry<'_>> = roots.iter().filter_map(|s| o.entry(s)).collect();
    let mut v = json!({
        "text": w.text,
        "translit": w.translit,
        "english": w.english,
        "strong": roots.join(" + "),
        "lemma": entries.iter().map(|e| e.lemma).collect::<Vec<_>>().join(" + "),
        "gloss": entries.iter().map(|e| e.gloss).collect::<Vec<_>>().join(" + "),
        "grammar": o.describe_grammar(w.grammar),
    });
    if w.grammar.starts_with('A') {
        v["aramaic"] = json!(true);
    }
    if w.in_title() {
        v["psalm_title"] = json!(true);
    }
    if let Some(e) = w.edition() {
        v["edition"] = json!(e);
    }
    v
}

fn lexicon(o: &Original, word: &str) -> Result<Value> {
    let entries = o.lookup(word);
    if entries.is_empty() {
        bail!("no Hebrew or Greek word {word:?}; use a Strong's number or lemma from original_words");
    }
    let clip = |s: &str| -> (String, bool) {
        match s.char_indices().nth(DEFINITION_CHARS) {
            Some((i, _)) => (format!("{} …", &s[..i]), true),
            None => (s.to_string(), false),
        }
    };
    // Senses of one number usually share their definitions: give them once.
    let mut given: Vec<(&str, &str, &str)> = Vec::new();
    let full: Vec<Value> = entries
        .iter()
        .take(LEXICON_ENTRIES)
        .map(|e| {
            let same = given
                .iter()
                .find(|(_, d, s)| *d == e.definition && *s == e.strongs_definition)
                .map(|(strong, ..)| *strong);
            if same.is_none() {
                given.push((e.strong, &e.definition, &e.strongs_definition));
            }
            let (definition, cut) = if same.is_some() { (String::new(), false) } else { clip(&e.definition) };
            let mut v = json!({
                "strong": e.strong,
                "lemma": e.lemma,
                "translit": e.translit,
                "grammar": e.grammar,
                "gloss": e.gloss,
                "occurrences": o.count(e.strong),
            });
            if !definition.is_empty() {
                v["definition"] = json!(definition);
            }
            if cut {
                v["definition_truncated"] = json!(true);
            }
            if let Some(strong) = same {
                v["definition_as_for"] = json!(strong);
            } else if !e.strongs_definition.is_empty() {
                v["strongs_definition"] = json!(e.strongs_definition);
            }
            v
        })
        .collect();
    let more: Vec<Value> = entries
        .iter()
        .skip(LEXICON_ENTRIES)
        .map(|e| json!({ "strong": e.strong, "gloss": e.gloss, "occurrences": o.count(e.strong) }))
        .collect();
    let mut out = json!({ "entries": full });
    if !more.is_empty() {
        out["other_senses"] = json!(more);
    }
    Ok(out)
}

/// "λόγος (logos)" for a word the model looks up by Strong's number; the query itself when it
/// isn't one the lexicon knows (or the text isn't loaded yet).
pub fn word_label(query: &str) -> String {
    if let Ok(o) = Original::bundled()
        && let Some(e) = o.lookup(query).first()
    {
        return format!("{} ({})", e.lemma, e.translit);
    }
    query.trim().to_string()
}

/// "Elijah" for a person or place the model looks up by id or Strong's number; the query itself
/// otherwise.
pub fn name_label(query: &str) -> String {
    if (query.contains('@') || original::normalize_strong(query).is_some())
        && let Ok(n) = Names::bundled()
        && let Some(e) = n.lookup(query).first()
    {
        return e.name().replace('_', " ");
    }
    query.trim().to_string()
}

/// A search hit for the model: the item's id to read and cite by, what it is and who it is from,
/// and where in it the passage is.
fn hit_json(h: &SearchHit) -> Value {
    let mut v = json!({ "item": h.item_uid, "kind": h.kind, "title": h.title });
    if let Some(source) = &h.source {
        v["from"] = source.as_str().into();
    }
    v["date"] = transcribe::day((h.date / 1000) as i64).into();
    if let Some(ms) = h.start_ms {
        v["at"] = transcribe::clock(ms).into();
    }
    if let Some(page) = h.page {
        v["page"] = page.into();
    }
    v["text"] = h.text.as_str().into();
    if h.anchored {
        v["anchored"] = true.into();
    }
    if h.unreviewed {
        v["unreviewed"] = true.into();
    }
    v
}

fn item_text_json(t: &ItemText) -> Value {
    let mut v = json!({ "item": t.uid, "kind": t.kind, "title": t.title, "text": t.text });
    if let Some(ms) = t.from_ms {
        v["from"] = transcribe::clock(ms).into();
    }
    if let Some(ms) = t.to_ms {
        v["to"] = transcribe::clock(ms).into();
    }
    v["truncated"] = t.truncated.into();
    v
}

fn scope(s: &str) -> Result<Scope> {
    Ok(match s.trim().to_lowercase().as_str() {
        "" | "all" => Scope::All,
        "ot" | "old testament" => Scope::OldTestament,
        "nt" | "new testament" => Scope::NewTestament,
        book => {
            let key = book_key(book);
            let b = exact_book(&key)
                .or_else(|| reference::book_candidates(&key, None).first().map(|(b, _)| *b))
                .with_context(|| format!("no book called {book:?}"))?;
            Scope::Book(b)
        }
    })
}

/// Words that say nothing about what to look for.
const STOP: &[&str] = &[
    "a", "about", "after", "all", "am", "an", "and", "any", "are", "as", "at", "be", "been", "but", "by",
    "can", "chapter", "could", "did", "do", "does", "for", "from", "had", "has", "have", "he", "her", "here",
    "him", "his", "how", "i", "if", "in", "into", "is", "it", "its", "me", "mean", "means", "my", "of", "on",
    "or", "our", "passage", "say", "says", "she", "so", "tell", "that", "the", "their", "them", "then",
    "there", "these", "they", "this", "those", "to", "us", "verse", "verses", "was", "we", "were", "what",
    "when", "where", "which", "who", "why", "will", "with", "would", "you", "your",
];

/// The stand-in until the assistant is connected: it says so, quotes the selected passage and
/// lists what a search of the tools' translation finds for the question's words.
pub fn local_reply(tools: &Tools, context: &Context, question: &str) -> Message {
    let t = &tools.translation;
    let mut out = format!(
        "The assistant isn't connected yet, so this is what Linger found in the {} on its own.",
        t.meta.abbrev
    );
    if let Some((a, b)) = context.selection {
        let r = Reference {
            book: a.book,
            chapter: a.chapter,
            verse: Some(a.verse),
            verse_end: (b.verse > a.verse).then_some(b.verse),
        };
        let (verses, _) = tools.passage(&r);
        let text = verses.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join(" ");
        out.push_str(&format!("\n\nYou selected “{text}” {{{{{}}}}}", r.display()));
    }
    let query = search_terms(question);
    if query.is_empty() {
        return Message::assistant(out);
    }
    let selected = context.selection.map(|(a, _)| a);
    // All the words if any verse has them all; otherwise the longest word alone.
    let mut hits = Vec::new();
    let mut used = query.join(" ");
    for q in [query.join(" "), query.iter().max_by_key(|w| w.len()).cloned().unwrap_or_default()] {
        if let Ok(v) = tools.run("search_bible", &json!({ "query": q, "limit": 6 }))
            && v["total"].as_u64().unwrap_or(0) > 0
        {
            hits = v["verses"].as_array().cloned().unwrap_or_default();
            used = q;
            break;
        }
    }
    let hits: Vec<&Value> = hits
        .iter()
        .filter(|h| {
            let r = h["reference"].as_str().and_then(reference::parse);
            r.map(|r| r.verse_ref()) != selected
        })
        .take(5)
        .collect();
    if hits.is_empty() {
        out.push_str(&format!("\n\nA search for “{used}” found nothing else."));
    } else {
        out.push_str(&format!("\n\nVerses with “{used}”:"));
        for h in hits {
            let (Some(r), Some(text)) = (h["reference"].as_str(), h["text"].as_str()) else { continue };
            out.push_str(&format!("\n\n“{text}” {{{{{r}}}}}"));
        }
    }
    Message::assistant(out)
}

/// The words of a question worth searching for: a quoted phrase if there is one, else the words
/// that aren't stop words or numbers.
pub fn search_terms(question: &str) -> Vec<String> {
    for (open, close) in [('"', '"'), ('“', '”'), ('\'', '\''), ('‘', '’')] {
        if let Some(a) = question.find(open)
            && let Some(len) = question[a + open.len_utf8()..].find(close)
        {
            let phrase = question[a + open.len_utf8()..a + open.len_utf8() + len].trim();
            if phrase.contains(' ') || phrase.len() > 2 {
                return phrase.split_whitespace().map(str::to_lowercase).collect();
            }
        }
    }
    question
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|w| w.trim_matches('\'').to_lowercase())
        .filter(|w| w.len() > 1 && !w.chars().all(|c| c.is_ascii_digit()) && !STOP.contains(&w.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Tools {
        Tools {
            translation: Arc::new(crate::bundled::load("kjv").unwrap()),
            library: Some(Arc::new(FakeLibrary::default())),
            church: None,
        }
    }

    /// A library of one sermon, that remembers what it was asked.
    #[derive(Default)]
    struct FakeLibrary {
        asked: std::sync::Mutex<Vec<Value>>,
        offline: bool,
    }

    const SERMON: &str = "a40671b1-af55-4640-b039-ebbb5b09900e";

    impl UserLibrary for FakeLibrary {
        fn search(&self, args: Value) -> Result<(Vec<SearchHit>, bool)> {
            self.asked.lock().unwrap().push(args.clone());
            let hit = SearchHit {
                item_uid: SERMON.into(),
                kind: crate::records::ItemKind::Sermon,
                title: "John 3 · Oct 19, 2026".into(),
                source: Some("John Piper, Desiring God".into()),
                date: 1_792_368_000_000,
                text: "Nicodemus came at night but did not stay in the dark.".into(),
                start_ms: Some(183_000),
                end_ms: Some(260_000),
                page: None,
                anchored: args["passage"].is_object(),
                unreviewed: true,
            };
            let found = args["query"].as_str().is_some_and(|q| q.contains("Nicodemus"));
            Ok((if found { vec![hit] } else { Vec::new() }, self.offline))
        }

        fn read(&self, args: Value) -> Result<Option<ItemText>> {
            self.asked.lock().unwrap().push(args.clone());
            if args["uid"] != SERMON {
                return Ok(None);
            }
            Ok(Some(ItemText {
                uid: SERMON.into(),
                kind: crate::records::ItemKind::Sermon,
                title: "John 3 · Oct 19, 2026".into(),
                text: "[14:00] The wind blows where it wishes; new birth is God's work.".into(),
                from_ms: Some(840_000),
                to_ms: Some(960_000),
                truncated: true,
            }))
        }

        fn counts(&self, chapter: Option<ChapterIdx>) -> Result<(usize, usize)> {
            Ok((2, chapter.map_or(0, |c| (c.display() == "John 3") as usize)))
        }
    }

    fn with(library: &Arc<FakeLibrary>) -> Tools {
        Tools { library: Some(library.clone()), ..tools() }
    }

    #[test]
    fn search_library_sends_the_passage_as_verse_ids_and_says_where_each_hit_is() {
        let fake = Arc::new(FakeLibrary::default());
        let t = with(&fake);
        let r = t
            .run(
                "search_library",
                &json!({ "query": "Nicodemus dark", "passage": "John 3:1-21", "limit": 99 }),
            )
            .unwrap();
        assert_eq!(r["total"], 1);
        let hit = &r["hits"][0];
        assert_eq!(hit["item"], SERMON);
        assert_eq!(hit["kind"], "sermon");
        assert_eq!(hit["from"], "John Piper, Desiring God");
        assert_eq!(hit["date"], "Oct 19, 2026");
        assert_eq!(hit["at"], "3:03");
        assert_eq!((&hit["anchored"], &hit["unreviewed"]), (&json!(true), &json!(true)));
        assert!(r.get("offline").is_none(), "{r}");
        // What the server was asked: the passage on the verse grid, the limit within its bounds.
        let asked = fake.asked.lock().unwrap().pop().unwrap();
        assert_eq!(asked["passage"], json!({ "from": 42_003_001, "to": 42_003_021 }));
        assert_eq!(asked["limit"], 50);
        assert_eq!(t.run("search_library", &json!({ "query": "morning wind" })).unwrap()["total"], 0);
        assert!(t.run("search_library", &json!({ "query": "x", "passage": "John 22" })).is_err());
        assert!(t.run("search_library", &json!({})).is_err());

        let offline = Arc::new(FakeLibrary { offline: true, ..Default::default() });
        let r = with(&offline).run("search_library", &json!({ "query": "Nicodemus" })).unwrap();
        assert!(r["offline"].as_str().unwrap().starts_with("The device is offline"), "{r}");

        let e =
            Tools { library: None, ..tools() }.run("search_library", &json!({ "query": "x" })).unwrap_err();
        assert!(e.to_string().contains("signed out, or asked for an answer from the Bible only"), "{e:#}");
    }

    #[test]
    fn read_item_sends_the_moment_in_milliseconds_and_stamps_the_window() {
        let fake = Arc::new(FakeLibrary::default());
        let t = with(&fake);
        let r = t.run("read_item", &json!({ "item": SERMON, "from": "14:00", "minutes": 5 })).unwrap();
        assert_eq!((&r["from"], &r["to"], &r["truncated"]), (&json!("14:00"), &json!("16:00"), &json!(true)));
        assert!(r["text"].as_str().unwrap().starts_with("[14:00] The wind"));
        let asked = fake.asked.lock().unwrap().pop().unwrap();
        assert_eq!(asked, json!({ "uid": SERMON, "fromMs": 840_000, "minutes": 5 }));
        let e = t.run("read_item", &json!({ "item": "ffff" })).unwrap_err();
        assert!(e.to_string().contains("no item"), "{e:#}");
        assert!(t.run("read_item", &json!({ "item": SERMON, "from": "soon" })).is_err());
        assert!(t.run("read_item", &json!({})).is_err());
    }

    #[test]
    fn item_citations_and_the_sermon_citations_of_older_chats() {
        let s =
            segments("As preached {{item a40671b1 12:30}}, noted {{item 3f1c2a9b}}, on {{item 3f1c p3}}.");
        assert_eq!(s[1], Segment::Item { uid: "a40671b1".into(), at_ms: Some(750_000), page: None });
        assert_eq!(s[3], Segment::Item { uid: "3f1c2a9b".into(), at_ms: None, page: None });
        assert_eq!(s[5], Segment::Item { uid: "3f1c".into(), at_ms: None, page: Some(3) });
        let s = segments(
            "As preached {{sermon a40671b1 12:30}}, not {{sermon nope 1:00}} or {{item a40671b1 p0}}.",
        );
        assert_eq!(s[1], Segment::Sermon { id: "a40671b1".into(), at_ms: 750_000 });
        assert_eq!(s.len(), 3, "an invalid citation stays text: {s:?}");
    }

    fn john(chapter: u8, verse: u8) -> VerseRef {
        VerseRef::new(BookId(42), chapter, verse)
    }

    #[test]
    fn splits_text_at_citations() {
        let s = segments("As in {{Numbers 21:9}}, so {{John 3:14-15}}. {{not a ref}} {{");
        assert_eq!(s.len(), 5, "{s:?}");
        assert_eq!(s[0], Segment::Text("As in ".into()));
        assert!(matches!(&s[1], Segment::Verse(r) if r.display() == "Numbers 21:9"));
        assert!(matches!(&s[3], Segment::Verse(r) if r.display() == "John 3:14–15"));
        assert_eq!(s[4], Segment::Text(". {{not a ref}} {{".into()));
    }

    #[test]
    fn citations_must_name_the_book() {
        // The typo-tolerant parser would read these as John and Revelation; a citation doesn't.
        for bad in ["{{jhon 3:16}}", "{{revalation 21}}", "{{Gospel 3:16}}"] {
            assert!(Message::assistant(bad).cited().is_empty(), "{bad}");
        }
        let m = Message::assistant("{{1 Cor 13:4–7}} {{Ps 23}} {{John 3:16}} {{John 3:16}}");
        let cited = m.cited();
        assert_eq!(cited.len(), 3, "repeats are dropped: {cited:?}");
        assert_eq!(cited[1].0.verse, 1);
        assert_eq!(cited[1].1.verse, 6, "a chapter citation covers the chapter");
        assert_eq!(cited[2], (john(3, 16), john(3, 16)));
    }

    #[test]
    fn get_passage_quotes_the_translation() {
        let v = tools().run("get_passage", &json!({ "reference": "John 3:16-17" })).unwrap();
        let verses = v["verses"].as_array().unwrap();
        assert_eq!(verses.len(), 2);
        assert_eq!(verses[0]["verse"], 16);
        assert!(verses[1]["text"].as_str().unwrap().starts_with("For God sent not his Son"));
        // No such passages; the forgiving parser would answer with Judges 2:1 and Job 22:1.
        for missing in ["Jude 2:1", "John 22:1", "John 3:99"] {
            assert!(tools().run("get_passage", &json!({ "reference": missing })).is_err(), "{missing}");
        }
        let long = tools().run("get_passage", &json!({ "reference": "Psalm 119" })).unwrap();
        assert_eq!(long["verses"].as_array().unwrap().len(), PASSAGE_LIMIT);
        assert_eq!(long["truncated"], true);
    }

    #[test]
    fn search_bible_scopes_and_limits() {
        let v = tools()
            .run("search_bible", &json!({ "query": "faith", "scope": "Hebrews", "limit": 3 }))
            .unwrap();
        let verses = v["verses"].as_array().unwrap();
        assert_eq!(verses.len(), 3);
        assert!(v["total"].as_u64().unwrap() > 3);
        assert!(verses.iter().all(|h| h["reference"].as_str().unwrap().starts_with("Hebrews")));
        assert!(tools().run("search_bible", &json!({ "query": "x", "scope": "Hezekiah" })).is_err());
    }

    #[test]
    fn original_words_give_each_word_its_lemma_gloss_and_grammar() {
        let v = tools().run("original_words", &json!({ "reference": "John 1:1" })).unwrap();
        assert_eq!(v["language"], "Greek");
        let words = v["verses"][0]["words"].as_array().unwrap();
        let logos = words.iter().find(|w| w["strong"] == "G3056").unwrap();
        assert_eq!(logos["lemma"], "λόγος");
        assert_eq!(logos["gloss"], "word");
        assert!(logos["grammar"].as_str().unwrap().contains("Nominative"), "{logos}");

        let v = tools().run("original_words", &json!({ "reference": "Genesis 1:1" })).unwrap();
        let first = &v["verses"][0]["words"][0];
        assert_eq!(first["strong"], "H7225G", "the root, not the prefix: {first}");
        assert!(first["grammar"].as_str().unwrap().starts_with("Preposition / "), "{first}");

        // A Psalm's title is part of verse 1, as in English Bibles.
        let v = tools().run("original_words", &json!({ "reference": "Psalm 23:1" })).unwrap();
        let words = v["verses"][0]["words"].as_array().unwrap();
        assert_eq!(words[0]["psalm_title"], true);
        assert!(words.iter().any(|w| w["strong"] == "H3068G" && w.get("psalm_title").is_none()));

        // The Johannine Comma is only in the Greek behind the KJV.
        let v = tools().run("original_words", &json!({ "reference": "1 John 5:7" })).unwrap();
        let heaven =
            v["verses"][0]["words"].as_array().unwrap().iter().find(|w| w["strong"] == "G3772").unwrap();
        assert!(heaven["edition"].as_str().unwrap().starts_with("only in the Greek behind the KJV"));

        let long = tools().run("original_words", &json!({ "reference": "Psalm 119:1-20" })).unwrap();
        assert_eq!(long["verses"].as_array().unwrap().len(), ORIGINAL_VERSES);
        assert_eq!(long["truncated"], true);
        assert!(tools().run("original_words", &json!({ "reference": "John 22:1" })).is_err());
    }

    #[test]
    fn lexicon_entries_by_number_and_by_lemma() {
        let v = tools().run("lexicon", &json!({ "word": "G3056" })).unwrap();
        assert!(v["entries"][0]["definition"].as_str().unwrap().starts_with("λόγος"));
        assert_eq!(
            tools().run("lexicon", &json!({ "word": "λόγος" })).unwrap()["entries"][0]["strong"],
            "G3056"
        );

        let v = tools().run("lexicon", &json!({ "word": "h1254" })).unwrap();
        let senses: Vec<&str> =
            v["entries"].as_array().unwrap().iter().map(|e| e["strong"].as_str().unwrap()).collect();
        assert!(
            senses.starts_with(&["H1254A", "H1254B"]),
            "create and be fat are different words: {senses:?}"
        );
        let create = &v["entries"][0];
        assert!(create["definition"].as_str().unwrap().starts_with("BDB: בָּרָא"), "{create}");
        assert!(create["strongs_definition"].as_str().unwrap().contains("create"));

        // Senses that share a definition give it once.
        let v = tools().run("lexicon", &json!({ "word": "H0430" })).unwrap();
        let entries = v["entries"].as_array().unwrap();
        assert!(entries[0]["definition"].is_string());
        assert!(entries[1..].iter().all(|e| e["definition_as_for"] == entries[0]["strong"]), "{v}");
        assert!(tools().run("lexicon", &json!({ "word": "G99999" })).is_err());
    }

    #[test]
    fn word_occurrences_count_render_and_quote() {
        // φιλέω: 13 times in John (25 in the New Testament).
        let v = tools()
            .run("word_occurrences", &json!({ "word": "G5368", "scope": "John", "limit": 50 }))
            .unwrap();
        assert_eq!(v["total"], 13);
        assert_eq!(v["by_book"], json!([{ "book": "John", "count": 13 }]));
        let verses = v["verses"].as_array().unwrap();
        let peter = verses.iter().find(|x| x["reference"] == "John 21:17").unwrap();
        assert!(
            peter["text"].as_str().unwrap().contains("lovest thou me"),
            "the tools' translation: {peter}"
        );
        assert_eq!(peter["original"].as_str().unwrap().matches(" … ").count(), 2, "three times in one verse");
        assert_eq!(v["truncated"], false);
        let nt =
            tools().run("word_occurrences", &json!({ "word": "G5368", "scope": "nt", "limit": 1 })).unwrap();
        assert_eq!(nt["total"], 25);
        assert_eq!(nt["truncated"], true);

        // A Hebrew root behind a prefix is rendered by its own English.
        let v = tools()
            .run("word_occurrences", &json!({ "word": "H0776G", "scope": "Genesis", "limit": 1 }))
            .unwrap();
        assert_eq!(v["verses"][0]["reference"], "Genesis 1:1");
        assert_eq!(v["verses"][0]["english"], "earth");
        assert_eq!(
            tools().run("word_occurrences", &json!({ "word": "H0776G", "scope": "nt" })).unwrap()["total"],
            0
        );
    }

    #[test]
    fn cross_references_rank_quote_and_skip_the_passage_itself() {
        let v = tools().run("cross_references", &json!({ "reference": "John 3:16", "limit": 5 })).unwrap();
        let refs = v["cross_references"].as_array().unwrap();
        assert_eq!(refs.len(), 5);
        assert_eq!(v["truncated"], true);
        assert_eq!(refs[0]["reference"], "Romans 5:8");
        assert!(refs[0]["text"].as_str().unwrap().contains("commendeth his love"), "{}", refs[0]);
        let votes: Vec<i64> = refs.iter().map(|r| r["votes"].as_i64().unwrap()).collect();
        assert!(votes.windows(2).all(|w| w[0] >= w[1]), "strongest first: {votes:?}");
        assert!(refs[0].get("from").is_none(), "one verse asked about: no need to say which");

        // A whole psalm: links between its own verses are left out, and each says where it's from.
        let v = tools().run("cross_references", &json!({ "reference": "Psalm 23", "limit": 30 })).unwrap();
        let refs = v["cross_references"].as_array().unwrap();
        assert!(refs.iter().all(|r| !r["reference"].as_str().unwrap().starts_with("Psalm 23:")), "{v}");
        assert!(refs.iter().all(|r| r["from"].as_str().unwrap().starts_with("Psalm 23:")));

        // A passage across chapters is quoted from where it starts.
        let v =
            tools().run("cross_references", &json!({ "reference": "Genesis 14:18", "limit": 30 })).unwrap();
        let hebrews = v["cross_references"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["reference"].as_str().unwrap().starts_with("Hebrews 6:20"))
            .unwrap();
        assert_eq!(hebrews["reference"], "Hebrews 6:20–7:3");
        assert!(hebrews["text"].as_str().unwrap().contains("order of Melchisedec"), "{hebrews}");
        assert_eq!(hebrews["text_truncated"], true);
        assert!(tools().run("cross_references", &json!({ "reference": "John 22:1" })).is_err());
    }

    #[test]
    fn people_and_places_tell_individuals_apart() {
        let t = tools();
        let v = t.run("people_and_places", &json!({ "name": "Zechariah" })).unwrap();
        assert!(v["total"].as_u64().unwrap() > 25, "{v}");
        assert_eq!(v["matches"].as_array().unwrap().len(), 1, "the most often named in full");
        assert!(v["matches"][0]["names"].is_array());
        let others = v["others"].as_array().unwrap();
        assert_eq!(others.len() + 1, v["total"].as_u64().unwrap() as usize);
        // John the Baptist's father is named in 9 verses, more than any other Zechariah.
        assert_eq!(v["matches"][0]["id"], "Zechariah@Luk.1.5");
        assert!(others.iter().any(|e| e["id"] == "Zechariah@2Ch.24.20"));
        assert!(others.iter().all(|e| e.get("names").is_none()), "the rest are summaries");
        let named = |e: &Value| e["verses_named_in"].as_u64().unwrap();
        assert!(others.iter().all(|e| named(e) <= named(&v["matches"][0])));

        // A reference picks out John the Baptist's father, in full.
        let v = t.run("people_and_places", &json!({ "name": "Zacharias", "reference": "Luke 1:5" })).unwrap();
        assert_eq!(v["total"], 1, "{v}");
        let z = &v["matches"][0];
        assert_eq!(z["id"], "Zechariah@Luk.1.5");
        assert_eq!(z["partners"], json!(["Elizabeth@Luk.1.5"]));
        assert_eq!(z["in_passage"], json!(["Luke 1:5"]));
        assert_eq!(z["names"][0]["original"], "Ζαχαρίας");

        // An id from a relation reads back, and the family links both ways.
        let aaron = &t.run("people_and_places", &json!({ "name": "Aaron" })).unwrap()["matches"][0];
        let father = aaron["father"].as_str().unwrap();
        let amram = &t.run("people_and_places", &json!({ "name": father })).unwrap()["matches"][0];
        assert!(amram["children"].as_array().unwrap().iter().any(|c| c == "Aaron@Exo.4.14"), "{amram}");

        // Strong's numbers as original_words gives them: the KJV's Elias is the prophet Elijah, and
        // the text's G5547 (Christ) finds the name STEPBible numbers G5547G.
        let elias = &t.run("people_and_places", &json!({ "name": "G2243" })).unwrap()["matches"][0];
        assert_eq!(elias["id"], "Elijah@1Ki.17.1");
        let christ = t.run("people_and_places", &json!({ "name": "G5547" })).unwrap();
        assert_eq!(christ["matches"][0]["id"], "Jesus@Isa.7.14");
        assert!(t.run("people_and_places", &json!({ "name": "Gandalf" })).is_err());
        assert!(t.run("people_and_places", &json!({})).is_err());
    }

    #[test]
    fn places_say_where_they_were_and_how_sure_scholars_are() {
        let v = tools().run("people_and_places", &json!({ "name": "Emmaus" })).unwrap();
        let locations = v["matches"][0]["locations"].as_array().unwrap();
        assert!(locations.len() > 1, "the candidates disagree: {v}");
        let confidence: Vec<u64> = locations.iter().map(|l| l["confidence"].as_u64().unwrap()).collect();
        assert!(confidence.windows(2).all(|w| w[0] >= w[1]), "most likely first: {confidence:?}");
        assert!(confidence[0] < 50, "no candidate is certain: {confidence:?}");
        let lat = locations[0]["lat"].as_f64().unwrap();
        let lon = locations[0]["lon"].as_f64().unwrap();
        // Near Jerusalem (31.78 N, 35.23 E), and not with latitude and longitude swapped.
        assert!((31.0..32.5).contains(&lat) && (34.5..35.5).contains(&lon), "{lat}, {lon}");

        // Who and what John 11:1 names, in order.
        let v = tools().run("people_and_places", &json!({ "reference": "John 11:1" })).unwrap();
        let all = v["matches"].as_array().unwrap().iter().chain(v["others"].as_array().into_iter().flatten());
        let ids: Vec<&str> = all.map(|e| e["id"].as_str().unwrap()).collect();
        for name in ["Lazarus@", "Bethany@", "Mary@", "Martha@"] {
            assert!(ids.iter().any(|id| id.starts_with(name)), "{name} in {ids:?}");
        }
    }

    #[test]
    fn picks_search_terms_from_a_question() {
        assert_eq!(search_terms("What does 'lifted up' mean in verse 14?"), ["lifted", "up"]);
        assert_eq!(search_terms("Why is the serpent in John 3?"), ["serpent", "john"]);
        assert!(search_terms("What does this verse mean?").is_empty());
    }

    #[test]
    fn the_stand_in_says_it_is_not_the_assistant_and_cites_real_verses() {
        let ctx = Context {
            chapter: ChapterIdx::new(BookId(42), 3).unwrap(),
            selection: Some((john(3, 14), john(3, 14))),
            reading: "NLT".into(),
            item: None,
        };
        let m = local_reply(&tools(), &ctx, "What does 'lifted up' mean in verse 14?");
        assert!(m.text.starts_with("The assistant isn't connected yet"));
        let cited = m.cited();
        assert_eq!(cited[0], (john(3, 14), john(3, 14)), "the selection comes first");
        assert!(cited.len() > 1, "{}", m.text);
        assert_eq!(cited.iter().filter(|c| **c == (john(3, 14), john(3, 14))).count(), 1);
        assert!(m.text.contains("And as Moses lifted up the serpent in the wilderness"));
    }

    #[test]
    fn ask_reads_only_bundled_public_domain_translations() {
        assert_eq!(ask_translation("web"), "web");
        assert_eq!(ask_translation("kjv"), "kjv");
        // A licensed, imported or unknown translation is never what Ask reads.
        for other in ["esv", "nlt", "net", "api-nasb", "my-import", ""] {
            assert_eq!(ask_translation(other), "bsb", "{other}");
        }
        for id in ASK_TRANSLATIONS {
            let meta = crate::bundled::load(id).unwrap().meta;
            assert!(meta.license.starts_with("Public Domain"), "{id}: {}", meta.license);
        }
    }

    #[test]
    fn the_model_hears_what_is_read_and_which_translation_the_tools_quote() {
        let ctx = Context {
            chapter: ChapterIdx::new(BookId(42), 3).unwrap(),
            selection: Some((john(3, 14), john(3, 16))),
            reading: "NLT".into(),
            item: None,
        };
        assert_eq!(ctx.describe(), "The user is reading John 3 in the NLT. They have selected John 3:14–16.");
        let about =
            Context { selection: None, item: Some(("1a2b3c4d".into(), "All things".into())), ..ctx.clone() };
        assert_eq!(
            about.describe(),
            "The user is reading John 3 in the NLT. They are asking about “All things”, item 1a2b3c4d in their library."
        );
        let translation = Arc::new(crate::bundled::load("bsb").unwrap());
        let tools = Tools { translation, library: None, church: None };
        assert_eq!(
            tools_note(&tools, Some(ctx.chapter)),
            "Passages you read and search come from the BSB (Berean Standard Bible)."
        );
        // How much library there is, how much of it touches the chapter, and whose church is the
        // user's own.
        let tools = Tools {
            library: Some(Arc::new(FakeLibrary::default())),
            church: Some("Grace Church".into()),
            ..tools
        };
        assert!(
            tools_note(&tools, Some(ctx.chapter)).ends_with(
                " The user's library has 2 items; 1 of them touches John 3. Their own church is Grace Church."
            ),
            "{}",
            tools_note(&tools, Some(ctx.chapter))
        );
        assert!(tools_note(&tools, None).contains(" The user's library has 2 items. "));
    }
}
