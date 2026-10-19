//! The Bible in its original languages, word by word: the Hebrew and Aramaic Old Testament and the
//! Greek New Testament, each word tagged with its dictionary form (an extended Strong's number),
//! its grammar and its English sense in that verse, with a lexicon of every word. Ask's word-study
//! tools read it, so what the assistant says about a Hebrew or Greek word comes from the text and
//! a lexicon rather than from memory.
//!
//! Built by `cargo xtask build-original` from STEPBible's tagged texts (TAHOT, TAGNT), brief
//! lexicons and grammar codes, with Greek definitions from Abbott-Smith (through STEPBible) and
//! Hebrew definitions from the Open Scriptures BDB outline and Strong's dictionary; provenance and
//! licences are in `LICENSES/README.md`. Verses are on the KJV grid. A Psalm title, verse 0 in
//! STEPBible, belongs to verse 1 as it does in English Bibles.
//!
//! The file is zstd-compressed UTF-8 text: a [`MAGIC`] line, then three sections.
//!
//! - `#morph`: `code \t description` for each grammar code.
//! - `#lexicon`: `strong \t lemma \t transliteration \t grammar \t gloss \t definition \t
//!   Strong's definition`. Line breaks inside a definition are stored as U+001F.
//! - `#words`: `=book chapter verse` (book index from 0) before each verse's words, then
//!   `text \t transliteration \t English \t strongs \t grammar \t flags` for each word. `strongs`
//!   is space-separated, with the root in braces when a Hebrew word has prefixes or suffixes.
//!   Flags: `t` in a Psalm title, `n` only in modern critical editions of the Greek, `k` only in
//!   the Greek text behind the KJV.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use anyhow::{Context as _, Result, bail};

use crate::books::{BookId, Testament, VerseRef};
use crate::search::Scope;

pub const MAGIC: &str = "linger-original 1";

/// Where a line break inside a definition is stored.
pub const LINE_BREAK: char = '\u{1f}';

type Span = (u32, u32);

/// The original-language text and its lexicon, decoded.
pub struct Original {
    buf: String,
    morph: HashMap<String, Span>,
    /// Lexicon line by Strong's number.
    lexicon: HashMap<String, Span>,
    /// Each word's line, in canonical order.
    words: Vec<Span>,
    /// `(verse key, first word)`, sorted; a verse's words run to the next verse's first word.
    verses: Vec<(u32, u32)>,
    /// Every word a Strong's number tags, prefixes and suffixes included.
    by_strong: HashMap<String, Vec<u32>>,
}

/// One word of the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word<'a> {
    pub verse: VerseRef,
    /// As written; in Hebrew, `/` separates prefixes and suffixes from the root.
    pub text: &'a str,
    pub translit: &'a str,
    /// What the word means here, in English (STEPBible's translation, after the BSB).
    pub english: &'a str,
    /// Strong's numbers, with the root in braces when there are several.
    pub strongs: &'a str,
    pub grammar: &'a str,
    pub flags: &'a str,
}

impl Word<'_> {
    /// The dictionary words this word is: its root, or every part of a Greek contraction.
    pub fn roots(&self) -> Vec<&str> {
        let parts: Vec<&str> = self.strongs.split(' ').filter(|s| !s.is_empty()).collect();
        let braced: Vec<&str> =
            parts.iter().filter_map(|p| p.strip_prefix('{').and_then(|p| p.strip_suffix('}'))).collect();
        if braced.is_empty() { parts } else { braced }
    }

    /// Every Strong's number on the word, prefixes and suffixes included.
    pub fn all_strongs(&self) -> impl Iterator<Item = &str> {
        self.strongs.split(' ').filter(|s| !s.is_empty()).map(|s| s.trim_matches(['{', '}']))
    }

    pub fn in_title(&self) -> bool {
        self.flags.contains('t')
    }

    /// Which Greek editions have the word, when not all of them do.
    pub fn edition(&self) -> Option<&'static str> {
        if self.flags.contains('n') {
            Some("only in modern critical editions (not in the Greek behind the KJV)")
        } else if self.flags.contains('k') {
            Some("only in the Greek behind the KJV (not in modern critical editions)")
        } else {
            None
        }
    }
}

/// A lexicon entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry<'a> {
    pub strong: &'a str,
    pub lemma: &'a str,
    pub translit: &'a str,
    pub grammar: &'a str,
    pub gloss: &'a str,
    /// Abbott-Smith for Greek, the BDB outline for Hebrew; may be empty.
    pub definition: String,
    /// Strong's own definition (Hebrew only); may be empty.
    pub strongs_definition: String,
}

impl Original {
    pub fn decode(compressed: &[u8]) -> Result<Self> {
        let raw = zstd::decode_all(compressed).context("decompressing the original-language text")?;
        Self::parse(String::from_utf8(raw).context("the original-language text isn't UTF-8")?)
    }

    pub fn parse(buf: String) -> Result<Self> {
        let mut o = Original {
            buf: String::new(),
            morph: HashMap::new(),
            lexicon: HashMap::new(),
            words: Vec::new(),
            verses: Vec::new(),
            by_strong: HashMap::new(),
        };
        let mut lines = LineSpans::new(&buf);
        if lines.next().map(|s| slice(&buf, s)) != Some(MAGIC) {
            bail!("not an original-language file");
        }
        let mut section = "";
        let mut verse: Option<u32> = None;
        for span in lines {
            let line = slice(&buf, span);
            if let Some(name) = line.strip_prefix('#') {
                section = match name {
                    "morph" => "morph",
                    "lexicon" => "lexicon",
                    "words" => "words",
                    other => bail!("unknown section {other:?}"),
                };
                continue;
            }
            match section {
                "morph" | "lexicon" => {
                    let key = line.split('\t').next().unwrap_or_default().to_string();
                    let map = if section == "morph" { &mut o.morph } else { &mut o.lexicon };
                    let value = if section == "morph" {
                        let at = key.len() as u32 + 1;
                        (span.0 + at, span.1)
                    } else {
                        span
                    };
                    map.insert(key, value);
                }
                "words" => {
                    if let Some(r) = line.strip_prefix('=') {
                        let n: Vec<u8> = r.split(' ').filter_map(|x| x.parse().ok()).collect();
                        let [b, c, v] = n[..] else { bail!("bad verse line {line:?}") };
                        let book =
                            BookId::new(b as usize).with_context(|| format!("bad book in {line:?}"))?;
                        let key = VerseRef::new(book, c, v).key();
                        if o.verses.last().is_some_and(|(k, _)| *k >= key) {
                            bail!("verses out of order at {line:?}");
                        }
                        o.verses.push((key, o.words.len() as u32));
                        verse = Some(key);
                        continue;
                    }
                    if verse.is_none() {
                        bail!("a word before the first verse");
                    }
                    let idx = o.words.len() as u32;
                    o.words.push(span);
                    let strongs = line.split('\t').nth(3).unwrap_or_default();
                    for s in strongs.split(' ').filter(|s| !s.is_empty()) {
                        let list = o.by_strong.entry(s.trim_matches(['{', '}']).to_string()).or_default();
                        if list.last() != Some(&idx) {
                            list.push(idx);
                        }
                    }
                }
                _ => bail!("a line before the first section"),
            }
        }
        o.buf = buf;
        Ok(o)
    }

    /// The bundled text, decoded the first time it's asked for (~0.2 s).
    #[cfg(feature = "bundled")]
    pub fn bundled() -> Result<&'static Original> {
        use std::sync::OnceLock;
        static BYTES: &[u8] =
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/original/original.zst"));
        static DECODED: OnceLock<std::result::Result<Original, String>> = OnceLock::new();
        DECODED
            .get_or_init(|| Original::decode(BYTES).map_err(|e| format!("{e:#}")))
            .as_ref()
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    pub fn word_count(&self) -> usize {
        self.words.len()
    }

    pub fn verse_count(&self) -> usize {
        self.verses.len()
    }

    /// The words of a verse, in order.
    pub fn words_in(&self, v: VerseRef) -> impl Iterator<Item = Word<'_>> {
        let range = match self.verses.binary_search_by_key(&v.key(), |(k, _)| *k) {
            Ok(i) => self.verse_words(i),
            Err(_) => 0..0,
        };
        range.map(move |w| self.word(w))
    }

    fn verse_words(&self, i: usize) -> Range<usize> {
        let start = self.verses[i].1 as usize;
        let end = self.verses.get(i + 1).map_or(self.words.len(), |(_, w)| *w as usize);
        start..end
    }

    /// The verse word `w` is in.
    fn verse_of(&self, w: u32) -> VerseRef {
        let i = self.verses.partition_point(|(_, first)| *first <= w) - 1;
        VerseRef::from_key(self.verses[i].0).expect("verse keys are valid")
    }

    fn word(&self, w: usize) -> Word<'_> {
        let line = slice(&self.buf, self.words[w]);
        let mut f = line.split('\t');
        let mut next = || f.next().unwrap_or_default();
        Word {
            verse: self.verse_of(w as u32),
            text: next(),
            translit: next(),
            english: next(),
            strongs: next(),
            grammar: next(),
            flags: next(),
        }
    }

    pub fn entry(&self, strong: &str) -> Option<Entry<'_>> {
        let span = *self.lexicon.get(strong)?;
        let mut f = slice(&self.buf, span).split('\t');
        let mut next = || f.next().unwrap_or_default();
        let unbreak = |s: &str| s.replace(LINE_BREAK, "\n");
        Some(Entry {
            strong: next(),
            lemma: next(),
            translit: next(),
            grammar: next(),
            gloss: next(),
            definition: unbreak(next()),
            strongs_definition: unbreak(next()),
        })
    }

    /// The lexicon entries a query names, in number order: one Strong's number ("G3056",
    /// "h430", "H1254A"), every sense of an undivided number ("H0430" also finds "H0430G"), or
    /// a lemma or transliteration exactly as the tools give it.
    pub fn lookup(&self, query: &str) -> Vec<Entry<'_>> {
        let keys: Vec<&str> = match normalize_strong(query) {
            Some(s) if self.lexicon.contains_key(&s) => {
                vec![self.lexicon.get_key_value(&s).map(|(k, _)| k.as_str()).unwrap_or_default()]
            }
            Some(s) => self.lexicon.keys().filter(|k| base(k) == s).map(String::as_str).collect(),
            None => {
                let q = query.trim().to_lowercase();
                self.lexicon
                    .iter()
                    .filter(|(_, span)| {
                        let mut f = slice(&self.buf, **span).split('\t').skip(1);
                        let (lemma, translit) = (f.next().unwrap_or_default(), f.next().unwrap_or_default());
                        lemma.trim() == query.trim() || translit.to_lowercase() == q
                    })
                    .map(|(k, _)| k.as_str())
                    .collect()
            }
        };
        let mut out: Vec<Entry<'_>> = keys.into_iter().filter_map(|k| self.entry(k)).collect();
        out.sort_by(|a, b| a.strong.cmp(b.strong));
        out
    }

    /// The words tagged with any of `strongs`, in canonical order.
    pub fn occurrences(&self, strongs: &[&str]) -> Vec<Word<'_>> {
        let mut idx: Vec<u32> =
            strongs.iter().filter_map(|s| self.by_strong.get(*s)).flatten().copied().collect();
        idx.sort_unstable();
        idx.dedup();
        idx.into_iter().map(|w| self.word(w as usize)).collect()
    }

    /// How many words a Strong's number tags.
    pub fn count(&self, strong: &str) -> usize {
        self.by_strong.get(strong).map_or(0, Vec::len)
    }

    /// A grammar code in words: "HVqp3ms" → "Verb : Qal (Simple, Active) Perfect …". Hebrew
    /// codes join a prefix, root and suffix with `/`; Greek contractions join with ` + `.
    pub fn describe_grammar(&self, code: &str) -> String {
        let lookup = |c: &str| self.morph.get(c).map(|s| slice(&self.buf, *s).to_string());
        if let Some(d) = lookup(code) {
            return d;
        }
        if code.contains(" + ") {
            return code
                .split(" + ")
                .map(|c| lookup(c).unwrap_or_else(|| c.to_string()))
                .collect::<Vec<_>>()
                .join(" + ");
        }
        // "HC/Ncmsa": the language letter starts only the first part.
        let lang = &code[..code.chars().next().map_or(0, char::len_utf8)];
        code.split('/')
            .enumerate()
            .map(|(i, part)| {
                let full = if i == 0 { part.to_string() } else { format!("{lang}{part}") };
                lookup(&full).unwrap_or_else(|| part.to_string())
            })
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

/// How often a word occurs in a scope, how it is rendered and where.
pub struct Study<'a> {
    pub total: usize,
    pub by_book: Vec<(BookId, usize)>,
    /// English renderings, most common first.
    pub renderings: Vec<(String, usize)>,
    pub words: Vec<Word<'a>>,
}

impl Original {
    /// Occurrences of the words `strongs` tags, within `scope`. Prefixes and suffixes that are
    /// separate Hebrew words ("and", "the") count where they are tagged.
    pub fn study(&self, strongs: &[&str], scope: Scope) -> Study<'_> {
        let words: Vec<Word<'_>> =
            self.occurrences(strongs).into_iter().filter(|w| in_scope(scope, w.verse.book)).collect();
        let mut by_book: BTreeMap<BookId, usize> = BTreeMap::new();
        let mut renderings: HashMap<String, usize> = HashMap::new();
        for w in &words {
            *by_book.entry(w.verse.book).or_default() += 1;
            let english = rendering(w, strongs);
            if !english.is_empty() {
                *renderings.entry(english).or_default() += 1;
            }
        }
        let mut renderings: Vec<(String, usize)> = renderings.into_iter().collect();
        renderings.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Study { total: words.len(), by_book: by_book.into_iter().collect(), renderings, words }
    }
}

/// The English for the part of a word that is one of `strongs`, without the translation's
/// brackets and end punctuation: "and/ the/ earth" → "earth" (or "the" for the article),
/// "[The] book" → "book", "loving;" → "loving".
pub fn rendering(w: &Word<'_>, strongs: &[&str]) -> String {
    let parts: Vec<&str> = w.english.split('/').map(str::trim).collect();
    let tags: Vec<&str> = w.all_strongs().collect();
    // A Hebrew word's English mirrors its parts.
    let english = match tags.iter().position(|t| strongs.contains(t)) {
        Some(i) if parts.len() == tags.len() && tags.len() > 1 => parts[i],
        _ => w.english.trim(),
    };
    let mut out = String::new();
    let mut depth = 0;
    for c in english.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let out =
        out.trim_matches(|c: char| c.is_ascii_punctuation() && c != '<' && c != '>' || "“”‘’—".contains(c));
    // Capitals stay: "God" and "god" are different renderings of אֱלֹהִים.
    if out.trim().is_empty() { english.to_string() } else { out.trim().to_string() }
}

pub fn in_scope(scope: Scope, book: BookId) -> bool {
    match scope {
        Scope::All => true,
        Scope::OldTestament => book.testament() == Testament::Old,
        Scope::NewTestament => book.testament() == Testament::New,
        Scope::Book(b) => b == book,
    }
}

/// "g25" → "G0025", "H430g" → "H0430G", "H 1254 a" → "H1254A"; `None` if it isn't a Strong's
/// number.
pub fn normalize_strong(s: &str) -> Option<String> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let mut chars = s.chars();
    let lang = chars.next()?.to_ascii_uppercase();
    if lang != 'H' && lang != 'G' {
        return None;
    }
    let rest = chars.as_str();
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let n: u32 = rest[..digits].parse().ok()?;
    let suffix = &rest[digits..];
    if suffix.len() > 1 || !suffix.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(format!("{lang}{n:04}{}", suffix.to_ascii_uppercase()))
}

/// A Strong's number without its sense letter: "H0430G" → "H0430".
pub fn base(strong: &str) -> &str {
    let digits = strong.bytes().skip(1).take_while(u8::is_ascii_digit).count();
    &strong[..(1 + digits).min(strong.len())]
}

fn slice(buf: &str, (a, b): Span) -> &str {
    &buf[a as usize..b as usize]
}

/// Byte spans of the lines of `s`, without their line breaks.
struct LineSpans<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> LineSpans<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, pos: 0 }
    }
}

impl Iterator for LineSpans<'_> {
    type Item = Span;

    fn next(&mut self) -> Option<Span> {
        if self.pos >= self.s.len() {
            return None;
        }
        let rest = &self.s.as_bytes()[self.pos..];
        let len = memchr::memchr(b'\n', rest).unwrap_or(rest.len());
        let span = (self.pos as u32, (self.pos + len) as u32);
        self.pos += len + 1;
        Some(span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "linger-original 1
#morph
HR\tPreposition
HNcfsa\tNoun Feminine Singular Absolute
HVqp3ms\tVerb Qal Perfect Third Singular Masculine
CONJ\tConjunction
P-1NS\tPersonal pronoun 1st Nominative Singular
#lexicon
H9003\tב\tbe\tH:Prep\tin\t\t
H1254A\tבָּרָא\tba.ra\tH:V\tto create\tshape, create\u{1f}Qal: create\tto create
H1254B\tבָּרָא\tba.ra\tH:V\tbe fat\tbe fat\t
H7225G\tרֵאשִׁית\tre.shit\tH:N-F\tbeginning\tfirst, beginning\t
G2532\tκαί\tkai\tG:CONJ\tand\tand, also\t
G1473\tἐγώ\tegō\tG:P\tI\tI\t
#words
=0 1 1
בְּ/רֵאשִׁית\tbe./re.Shit\tin/ beginning\tH9003 {H7225G}\tHR/Ncfsa\t
בָּרָא\tba.Ra\the created\tH1254A\tHVqp3ms\t
=18 3 1
מִזְמוֹר\tmiz.Mor\ta psalm\tH4210\tHNcmsa\tt
=42 10 27
κἀγὼ\tkagō\tand I\tG2532 G1473\tCONJ + P-1NS\tn
";

    fn sample() -> Original {
        Original::parse(SAMPLE.to_string()).unwrap()
    }

    #[test]
    fn reads_words_by_verse() {
        let o = sample();
        let genesis = VerseRef::new(BookId::GENESIS, 1, 1);
        let words: Vec<Word<'_>> = o.words_in(genesis).collect();
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].roots(), ["H7225G"]);
        assert_eq!(words[0].all_strongs().collect::<Vec<_>>(), ["H9003", "H7225G"]);
        assert_eq!(words[1].verse, genesis);
        assert_eq!(o.words_in(VerseRef::new(BookId::GENESIS, 1, 2)).count(), 0);
        let title: Vec<Word<'_>> = o.words_in(VerseRef::new(BookId::PSALMS, 3, 1)).collect();
        assert!(title[0].in_title());
        // The last verse runs to the end of the words.
        let john: Vec<Word<'_>> = o.words_in(VerseRef::new(BookId(42), 10, 27)).collect();
        assert_eq!(john[0].roots(), ["G2532", "G1473"], "a contraction is both words");
        assert!(john[0].edition().unwrap().starts_with("only in modern"));
    }

    #[test]
    fn looks_up_strongs_numbers_and_their_senses() {
        let o = sample();
        assert_eq!(o.lookup("h1254a")[0].gloss, "to create");
        let both = o.lookup("H1254");
        assert_eq!(both.iter().map(|e| e.strong).collect::<Vec<_>>(), ["H1254A", "H1254B"]);
        assert_eq!(o.lookup("H7225")[0].strong, "H7225G", "an undivided number finds its senses");
        assert_eq!(o.lookup("H1254A")[0].definition, "shape, create\nQal: create");
        assert_eq!(o.lookup("καί")[0].strong, "G2532");
        assert_eq!(o.lookup("Kai")[0].strong, "G2532");
        assert!(o.lookup("G9999").is_empty());
    }

    #[test]
    fn finds_occurrences_of_prefixes_and_roots() {
        let o = sample();
        assert_eq!(o.count("H9003"), 1);
        assert_eq!(o.occurrences(&["G1473"])[0].verse, VerseRef::new(BookId(42), 10, 27));
        let s = o.study(&["H7225G", "H1254A"], Scope::All);
        assert_eq!(s.total, 2);
        assert_eq!(s.renderings, [("beginning".to_string(), 1), ("he created".to_string(), 1)]);
        assert_eq!(o.study(&["H7225G"], Scope::NewTestament).total, 0);
    }

    #[test]
    fn describes_grammar_codes() {
        let o = sample();
        assert_eq!(o.describe_grammar("HVqp3ms"), "Verb Qal Perfect Third Singular Masculine");
        assert_eq!(o.describe_grammar("HR/Ncfsa"), "Preposition / Noun Feminine Singular Absolute");
        assert_eq!(
            o.describe_grammar("CONJ + P-1NS"),
            "Conjunction + Personal pronoun 1st Nominative Singular"
        );
        assert_eq!(o.describe_grammar("XYZ"), "XYZ");
    }

    #[test]
    fn normalizes_strongs_numbers() {
        assert_eq!(normalize_strong("g25").as_deref(), Some("G0025"));
        assert_eq!(normalize_strong("H430g").as_deref(), Some("H0430G"));
        assert_eq!(normalize_strong("H 1254 a").as_deref(), Some("H1254A"));
        assert_eq!(normalize_strong("G10005").as_deref(), Some("G10005"));
        for bad in ["logos", "G", "Gab", "G12AB", "X123"] {
            assert_eq!(normalize_strong(bad), None, "{bad}");
        }
        assert_eq!(base("H0430G"), "H0430");
        assert_eq!(base("G3056"), "G3056");
    }

    #[test]
    fn renderings_drop_prefixes_and_brackets() {
        let o = sample();
        let w = o.words_in(VerseRef::new(BookId::GENESIS, 1, 1)).next().unwrap();
        assert_eq!(rendering(&w, &["H7225G"]), "beginning");
        assert_eq!(rendering(&w, &["H9003"]), "in", "a prefix's own English");
        let mut w = w.clone();
        w.english = "[The] book";
        w.strongs = "G0976";
        assert_eq!(rendering(&w, &["G0976"]), "book");
        w.english = "[is]";
        assert_eq!(rendering(&w, &["G0976"]), "[is]", "a word that is all brackets keeps them");
    }
}
