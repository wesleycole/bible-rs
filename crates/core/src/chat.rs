//! Conversations with the assistant: messages, the citations inside them, and the tools the
//! assistant runs on the device.
//!
//! The model never writes Scripture from memory. It cites references as `{{John 3:16}}` (or a
//! range, `{{John 3:16-18}}`); the app turns each into a chip that opens the verse in the reader,
//! and any verse text it shows comes from the user's own translation through [`Tools`]. The same
//! tools serve the model's tool calls and the offline stand-in, [`local_reply`].

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::books::{BookId, ChapterIdx, VerseRef};
use crate::model::Translation;
use crate::reference::{self, Reference};
use crate::search::{self, Scope};

/// What the user is looking at when they ask: sent with every question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub chapter: ChapterIdx,
    /// An inclusive verse range.
    pub selection: Option<(VerseRef, VerseRef)>,
}

impl Context {
    /// For the model: "The user is reading John 3 in the KJV and has selected John 3:14."
    pub fn describe(&self, t: &Translation) -> String {
        let mut s = format!("The user is reading {} in the {}.", self.chapter.display(), t.meta.abbrev);
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
        match parse_citation(inner) {
            Some(r) => {
                if !pending.is_empty() {
                    out.push(Segment::Text(std::mem::take(&mut pending)));
                }
                out.push(Segment::Verse(r));
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

/// The tools the assistant can call, run against the user's reading translation.
pub struct Tools {
    pub translation: Arc<Translation>,
}

/// The most verses one `get_passage` call returns, so a chapter of Psalm 119 can't flood the
/// conversation.
const PASSAGE_LIMIT: usize = 60;

impl Tools {
    /// Tool definitions in the Anthropic Messages API's `tools` format.
    pub fn definitions() -> Value {
        json!([
            {
                "name": "search_bible",
                "description": "Full-text search of the user's Bible translation. Every word must appear \
                    (as a word prefix); use \"quotes\" for an exact phrase and -word to exclude. Returns \
                    matching verses with their text, in canonical order.",
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
                "name": "get_passage",
                "description": "The text of a passage in the user's translation: a verse (\"John 3:16\"), a \
                    range (\"John 3:16-21\") or a chapter (\"Psalm 23\"). Quote Scripture only from this.",
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
            other => bail!("unknown tool {other}"),
        }
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
/// lists what a search of the reading translation finds for the question's words.
pub fn local_reply(tools: &Tools, context: &Context, question: &str) -> Message {
    let t = &tools.translation;
    let mut out = format!(
        "The assistant isn't connected yet, so this is what Linger found in your {} on its own.",
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
        Tools { translation: Arc::new(crate::bundled::load("kjv").unwrap()) }
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
        };
        let m = local_reply(&tools(), &ctx, "What does 'lifted up' mean in verse 14?");
        assert!(m.text.starts_with("The assistant isn't connected yet"));
        let cited = m.cited();
        assert_eq!(cited[0], (john(3, 14), john(3, 14)), "the selection comes first");
        assert!(cited.len() > 1, "{}", m.text);
        assert_eq!(cited.iter().filter(|c| **c == (john(3, 14), john(3, 14))).count(), 1);
        assert!(m.text.contains("And as Moses lifted up the serpent in the wilderness"));
    }
}
