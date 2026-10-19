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
//! Sermons the user recorded are cited the same way, by recording and moment:
//! `{{sermon 1a2b3c4d 12:30}}`.

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::books::{BookId, ChapterIdx, VerseRef};
use crate::model::Translation;
use crate::reference::{self, Reference};
use crate::search::{self, Scope};
use crate::transcribe::{self, Transcript};

/// The translations Ask may read: bundled and public domain, so their text may go to a model.
/// The first is the default.
pub const ASK_TRANSLATIONS: [&str; 3] = ["bsb", "web", "kjv"];

/// The translation Ask's tools read, from the user's preference: one of [`ASK_TRANSLATIONS`],
/// and the BSB for anything else (a licensed or imported translation is never used).
pub fn ask_translation(preference: &str) -> &'static str {
    ASK_TRANSLATIONS.iter().copied().find(|id| *id == preference).unwrap_or(ASK_TRANSLATIONS[0])
}

/// For the model, with every question: "Passages you read and search come from the BSB (Berean
/// Standard Bible)."
pub fn tools_note(t: &Translation) -> String {
    format!("Passages you read and search come from the {} ({}).", t.meta.abbrev, t.meta.name)
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
}

impl Context {
    /// For the model: "The user is reading John 3 in the NLT. They have selected John 3:14."
    pub fn describe(&self) -> String {
        let mut s = format!("The user is reading {} in the {}.", self.chapter.display(), self.reading);
        if let Some((a, b)) = self.selection {
            s.push_str(&format!(" They have selected {}.", reference::format_range(a, b)));
        }
        s
    }
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
    /// A moment in a recorded sermon: the recording id (or its first characters) and where.
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
        match parse_citation(inner).map(Segment::Verse).or_else(|| parse_sermon_citation(inner)) {
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

/// `sermon 1a2b3c4d 12:30`.
fn parse_sermon_citation(inner: &str) -> Option<Segment> {
    let mut words = inner.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("sermon") {
        return None;
    }
    let id = words.next()?.to_string();
    let at_ms = transcribe::parse_clock(words.next()?)?;
    (words.next().is_none() && id.len() >= 4 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        .then_some(Segment::Sermon { id, at_ms })
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
/// [`ask_translation`]) and the sermons the user has recorded.
pub struct Tools {
    pub translation: Arc<Translation>,
    /// Transcribed sermons, newest first.
    pub sermons: Arc<Vec<Transcript>>,
}

/// The most verses one `get_passage` call returns, so a chapter of Psalm 119 can't flood the
/// conversation.
const PASSAGE_LIMIT: usize = 60;
/// Words of transcript `read_sermon` returns at most: about 15 minutes of preaching.
const SERMON_WORDS: usize = 2500;
/// The first characters of a recording id: enough to tell a person's sermons apart.
const SHORT_ID: usize = 8;

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
                "name": "search_sermons",
                "description": "Searches the transcripts of sermons the user recorded at their church (newest \
                    first). Every word must appear in a passage of the sermon. Returns excerpts with the \
                    sermon's id and the moment they were said. Use it whenever the user asks what their \
                    church, pastor or a sermon taught.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 20 }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            },
            {
                "name": "read_sermon",
                "description": "Reads a recorded sermon's transcript from a moment on (\"12:30\"; the start by \
                    default), up to about 15 minutes. With no sermon id, lists the recorded sermons.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "sermon": { "type": "string", "description": "A sermon id from search_sermons or the list." },
                        "from": { "type": "string" },
                        "minutes": { "type": "integer", "minimum": 1, "maximum": 15 }
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
            "search_sermons" => {
                let query = input["query"].as_str().context("query is required")?;
                let limit = input["limit"].as_u64().unwrap_or(8).clamp(1, 20) as usize;
                let hits = self.search_sermons(query);
                let shown: Vec<Value> = hits.iter().take(limit).cloned().collect();
                Ok(json!({ "sermons": self.sermons.len(), "total": hits.len(), "hits": shown }))
            }
            "read_sermon" => match input["sermon"].as_str() {
                None => Ok(json!({ "sermons": self.sermons.iter().map(sermon_summary).collect::<Vec<_>>() })),
                Some(id) => {
                    let t = self.sermon(id)?;
                    let from = match input["from"].as_str() {
                        Some(f) => {
                            transcribe::parse_clock(f).with_context(|| format!("not a time: {f:?}"))?
                        }
                        None => 0,
                    };
                    let minutes = input["minutes"].as_u64().unwrap_or(10).clamp(1, 15);
                    Ok(read_sermon(t, from, from + minutes * 60_000))
                }
            },
            other => bail!("unknown tool {other}"),
        }
    }

    /// The transcribed sermon whose id starts with `id`.
    pub fn sermon(&self, id: &str) -> Result<&Transcript> {
        let id = id.trim();
        let mut found = self.sermons.iter().filter(|t| !id.is_empty() && t.recording_id.starts_with(id));
        match (found.next(), found.next()) {
            (Some(t), None) => Ok(t),
            (Some(_), Some(_)) => bail!("more than one sermon's id starts with {id:?}"),
            (None, _) if self.sermons.is_empty() => bail!("the user hasn't recorded any sermons yet"),
            (None, _) => bail!("no sermon with id {id:?}"),
        }
    }

    /// Passages where every word of `query` is said, newest sermon first: each an excerpt of about
    /// 60 words around the first match, timed by where it falls in its turn of speech.
    fn search_sermons(&self, query: &str) -> Vec<Value> {
        let terms: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '\'')
            .map(str::to_lowercase)
            .filter(|w| !w.is_empty() && !STOP.contains(&w.as_str()))
            .collect();
        if terms.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for t in self.sermons.iter() {
            for seg in &t.segments {
                let words: Vec<&str> = seg.text.split_whitespace().collect();
                let norm: Vec<String> = words
                    .iter()
                    .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
                    .collect();
                // Match within a window of 80 words rather than anywhere in a long turn.
                let window = 80.min(norm.len());
                let Some(at) = (0..=norm.len() - window).step_by(20).find(|&i| {
                    terms.iter().all(|term| norm[i..i + window].iter().any(|w| w.starts_with(term.as_str())))
                }) else {
                    continue;
                };
                let first = (at..at + window).find(|&i| norm[i].starts_with(terms[0].as_str())).unwrap_or(at);
                let (a, b) = (first.saturating_sub(25), (first + 35).min(words.len()));
                let span = seg.end_ms.saturating_sub(seg.start_ms);
                let at_ms = seg.start_ms + span * a as u64 / words.len().max(1) as u64;
                let mut excerpt = words[a..b].join(" ");
                if a > 0 {
                    excerpt.insert_str(0, "… ");
                }
                if b < words.len() {
                    excerpt.push_str(" …");
                }
                out.push(json!({
                    "sermon": short_id(t),
                    "title": t.title(),
                    "at": transcribe::clock(at_ms),
                    "speaker": seg.speaker,
                    "excerpt": excerpt,
                }));
            }
        }
        out
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

fn short_id(t: &Transcript) -> &str {
    &t.recording_id[..SHORT_ID.min(t.recording_id.len())]
}

fn sermon_summary(t: &Transcript) -> Value {
    json!({ "sermon": short_id(t), "title": t.title(), "length": transcribe::clock(t.duration_ms) })
}

/// The turns of speech that overlap `from..to`, up to [`SERMON_WORDS`] words.
fn read_sermon(t: &Transcript, from: u64, to: u64) -> Value {
    let mut words = 0;
    let mut truncated = false;
    let mut turns = Vec::new();
    for seg in t.segments.iter().filter(|s| s.end_ms > from && s.start_ms < to) {
        let n = seg.text.split_whitespace().count();
        if words + n > SERMON_WORDS && !turns.is_empty() {
            truncated = true;
            break;
        }
        words += n;
        turns
            .push(json!({ "at": transcribe::clock(seg.start_ms), "speaker": seg.speaker, "text": seg.text }));
    }
    json!({
        "sermon": short_id(t),
        "title": t.title(),
        "length": transcribe::clock(t.duration_ms),
        "turns": turns,
        "truncated": truncated || t.segments.iter().any(|s| s.start_ms >= to),
    })
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
            sermons: Arc::new(vec![sermon()]),
        }
    }

    fn sermon() -> Transcript {
        let seg = |start_ms, end_ms, text: &str| transcribe::Segment {
            start_ms,
            end_ms,
            speaker: Some("A".into()),
            text: text.into(),
        };
        // A long turn: the match is about 3/4 of the way through it.
        let mut long = "word ".repeat(150);
        long.push_str("Nicodemus came at night but did not stay in the dark. ");
        long.push_str(&"word ".repeat(42));
        Transcript {
            recording_id: "a40671b1-af55-4640-b039-ebbb5b09900e".into(),
            recorded_at: 1_792_368_000,
            duration_ms: 1_800_000,
            language: Some("en".into()),
            segments: vec![
                seg(0, 60_000, "Good morning, church. Turn with me to John chapter three."),
                seg(60_000, 260_000, &long),
                seg(900_000, 960_000, "The wind blows where it wishes; new birth is God's work."),
            ],
        }
    }

    #[test]
    fn sermon_search_finds_the_moment_inside_a_long_turn() {
        let t = tools();
        let r = t.run("search_sermons", &json!({ "query": "Nicodemus dark" })).unwrap();
        assert_eq!(r["total"], 1);
        let hit = &r["hits"][0];
        assert_eq!(hit["sermon"], "a40671b1");
        assert!(hit["excerpt"].as_str().unwrap().contains("did not stay in the dark"));
        // The excerpt starts 25 words before word 150 of 203 (150 + 11 + 42), in a 200 s turn
        // that starts at 60 s: 60 + 200 × 125 / 203 = 183.1 s.
        assert_eq!(hit["at"], "3:03");
        // Words spread across turns don't match as one passage.
        assert_eq!(t.run("search_sermons", &json!({ "query": "morning wind" })).unwrap()["total"], 0);
    }

    #[test]
    fn reading_a_sermon_from_a_moment() {
        let t = tools();
        let r = t.run("read_sermon", &json!({ "sermon": "a406", "from": "14:00", "minutes": 5 })).unwrap();
        let turns = r["turns"].as_array().unwrap();
        assert_eq!(turns.len(), 1, "only the turn inside 14:00–19:00: {r}");
        assert_eq!(turns[0]["at"], "15:00");
        let list = t.run("read_sermon", &json!({})).unwrap();
        assert_eq!(list["sermons"][0]["title"], "Sermon · Oct 19, 2026");
        assert!(t.run("read_sermon", &json!({ "sermon": "ffff" })).is_err());
    }

    #[test]
    fn sermon_citations() {
        let s = segments("As preached {{sermon a40671b1 12:30}}, not {{sermon nope 1:00}}.");
        assert_eq!(s[1], Segment::Sermon { id: "a40671b1".into(), at_ms: 750_000 });
        assert_eq!(s.len(), 3, "an invalid sermon citation stays text: {s:?}");
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
        };
        assert_eq!(ctx.describe(), "The user is reading John 3 in the NLT. They have selected John 3:14–16.");
        let bsb = crate::bundled::load("bsb").unwrap();
        assert_eq!(
            tools_note(&bsb),
            "Passages you read and search come from the BSB (Berean Standard Bible)."
        );
    }
}
