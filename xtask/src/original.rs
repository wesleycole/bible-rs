//! `cargo xtask build-original`: the Hebrew OT and Greek NT word by word with a lexicon, for
//! Ask's word-study tools (see `linger_core::original` for the file format).
//!
//! Sources, pinned by commit and SHA-256 and downloaded into `.sources/`:
//!
//! - STEPBible-Data (Tyndale House, CC BY 4.0): the tagged texts TAHOT and TAGNT, the brief
//!   lexicons TBESH and TBESG, and the grammar codes TEHMC and TEGMC.
//! - Open Scriptures HebrewLexicon (CC BY 4.0; the BDB and Strong's text itself is public
//!   domain): the BDB outline and Strong's Hebrew dictionary.
//!
//! What is kept and what is left out, and why, is in `LICENSES/README.md`. In short: TBESH's
//! Hebrew definitions are Online Bible's Abridged BDB, which needs Online Bible's permission, so
//! Hebrew definitions come from Open Scriptures instead; Greek definitions that STEPBible took
//! from Middle Liddell (CC BY-SA) or Mounce are dropped, leaving the gloss; TAGNT's Spanish and
//! sub-meaning columns are not used.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use linger_core::books::{BookId, ChapterIdx, VerseRef};
use linger_core::original::{self, LINE_BREAK, MAGIC};
use linger_core::xml::{self, Tok};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

const STEP_COMMIT: &str = "ed476a8ee6e3f7b3a46fc48af6c7dabc02387b1d";
const OSHB_COMMIT: &str = "21c9add13bc727d3a951361778e97e3ff7afd1ce";

struct File {
    repo: &'static str,
    commit: &'static str,
    path: &'static str,
    sha256: &'static str,
}

const fn step(path: &'static str, sha256: &'static str) -> File {
    File { repo: "STEPBible/STEPBible-Data", commit: STEP_COMMIT, path, sha256 }
}

const fn oshb(path: &'static str, sha256: &'static str) -> File {
    File { repo: "openscriptures/HebrewLexicon", commit: OSHB_COMMIT, path, sha256 }
}

const TAHOT: [File; 4] = [
    step(
        "Translators Amalgamated OT+NT/TAHOT Gen-Deu - Translators Amalgamated Hebrew OT - STEPBible.org CC BY.txt",
        "e9b8546ee48fe0bfc57c3b70f5f40e98d96580e803526d19026224e31753368b",
    ),
    step(
        "Translators Amalgamated OT+NT/TAHOT Jos-Est - Translators Amalgamated Hebrew OT - STEPBible.org CC BY.txt",
        "195fee1dc3653bab33701f170734eb894ed647c10cd08cc61749375fe8b73775",
    ),
    step(
        "Translators Amalgamated OT+NT/TAHOT Job-Sng - Translators Amalgamated Hebrew OT - STEPBible.org CC BY.txt",
        "bb508af3af07822fca84cc43a0eba79eb70eecf672aabc0245b8d8cecf329d96",
    ),
    step(
        "Translators Amalgamated OT+NT/TAHOT Isa-Mal - Translators Amalgamated Hebrew OT - STEPBible.org CC BY.txt",
        "f3ded203d2a74d6368932c97ae550d1d0754b271af491dc0dedf36fe3ba0bcc5",
    ),
];
const TAGNT: [File; 2] = [
    step(
        "Translators Amalgamated OT+NT/TAGNT Mat-Jhn - Translators Amalgamated Greek NT - STEPBible.org CC-BY.txt",
        "ab8eaaeb68e17a1dcfa34e1e9350358f22f03bc2a97244d848750ad81044bc8e",
    ),
    step(
        "Translators Amalgamated OT+NT/TAGNT Act-Rev - Translators Amalgamated Greek NT - STEPBible.org CC-BY.txt",
        "524e32375361e6d3fa2f7ef00b87605fdc4317a762f395651a05fdc31ad031b7",
    ),
];
const TBESH: File = step(
    "Lexicons/TBESH - Translators Brief lexicon of Extended Strongs for Hebrew - STEPBible.org CC BY.txt",
    "464dccadd95fd8620dd05fa0d7a4caba58ec3c4d5db3ebf38e43d046ca25b591",
);
const TBESG: File = step(
    "Lexicons/TBESG - Translators Brief lexicon of Extended Strongs for Greek - STEPBible.org CC BY.txt",
    "312f723d7b8ef263bbdfb0451c9b8057125804dfff390b6f8544cff2a84b57f4",
);
const TEHMC: File = step(
    "Morphology codes/TEHMC - Translators Expansion of Hebrew Morphology Codes - STEPBible.org CC BY.txt",
    "78779bea824b31d4467dec0161d547481c86f266bc39def12cd11dc7dcbe6da7",
);
const TEGMC: File = step(
    "Morphology codes/TEGMC - Translators Expansion of Greek Morphhology Codes - STEPBible.org CC BY.txt",
    "5f0416f7617019a6082285214903bde569a980d5fd3b88b8d7020d944e94de82",
);
const BDB: File =
    oshb("BrownDriverBriggs.xml", "2b52658a4323d91674cda4090ab8b3ebddfff640f4f18143c28300e80b2c38f8");
const STRONG: File =
    oshb("HebrewStrong.xml", "a628f4f89f8bdaf2483fd3faf1abc8653cc6717758dfc9f24beb7571d9bdd0c4");
const LEXICAL_INDEX: File =
    oshb("LexicalIndex.xml", "8f7a605c58899d2f44430149c143c00903976e1e91232476677972a69e5bc85f");
const AUG_INDEX: File =
    oshb("AugIndex.xml", "e7217ca8ff8ff3f21f9cf1bbe87411adf55f6aa88bcf5ed9ddc886cc6b160c5d");

/// STEPBible's book abbreviations, in canonical order.
const BOOKS: [&str; 66] = [
    "Gen", "Exo", "Lev", "Num", "Deu", "Jos", "Jdg", "Rut", "1Sa", "2Sa", "1Ki", "2Ki", "1Ch", "2Ch", "Ezr",
    "Neh", "Est", "Job", "Psa", "Pro", "Ecc", "Sng", "Isa", "Jer", "Lam", "Ezk", "Dan", "Hos", "Jol", "Amo",
    "Oba", "Jon", "Mic", "Nam", "Hab", "Zep", "Hag", "Zec", "Mal", "Mat", "Mrk", "Luk", "Jhn", "Act", "Rom",
    "1Co", "2Co", "Gal", "Eph", "Php", "Col", "1Th", "2Th", "1Ti", "2Ti", "Tit", "Phm", "Heb", "Jas", "1Pe",
    "2Pe", "1Jn", "2Jn", "3Jn", "Jud", "Rev",
];

pub fn build(root: &Path) -> Result<()> {
    let read = |f: &File| -> Result<String> {
        let path = fetch(root, f)?;
        let s = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
        Ok(s.trim_start_matches('\u{feff}').to_string())
    };

    let mut morph = BTreeMap::new();
    for f in [&TEHMC, &TEGMC] {
        morph_codes(&read(f)?, &mut morph);
    }

    let hebrew_defs =
        HebrewDefinitions::new(&read(&BDB)?, &read(&STRONG)?, &read(&LEXICAL_INDEX)?, &read(&AUG_INDEX)?);
    let mut lexicon: BTreeMap<String, Lex> = BTreeMap::new();
    let mut stats = Stats::default();
    lexicon_rows(&read(&TBESH)?, &mut lexicon, |row, lex| {
        if let Some((d, s)) = hebrew_defs.get(&row[0], &row[5]) {
            lex.definition = d;
            lex.strongs_definition = s;
        }
    });
    lexicon_rows(&read(&TBESG)?, &mut lexicon, |row, lex| {
        let meaning = row.get(7).map(String::as_str).unwrap_or_default();
        // Middle Liddell (CC BY-SA) and Mounce (by permission to STEPBible) aren't ours to ship.
        if !meaning.contains("(ML)") && !meaning.contains("(MT)") {
            lex.definition = tidy(&html_to_text(meaning).replace("(AS)", ""));
        }
    });

    let mut verses: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for f in &TAHOT {
        hebrew_words(&read(f)?, &mut verses, &mut stats)?;
    }
    for f in &TAGNT {
        greek_words(&read(f)?, &mut verses, &mut stats)?;
    }

    // Only the lexicon entries the text uses.
    let mut used = HashSet::new();
    for line in verses.values().flatten() {
        let strongs = line.split('\t').nth(3).unwrap_or_default();
        for s in strongs.split(' ') {
            used.insert(s.trim_matches(['{', '}']).to_string());
        }
    }
    // A few tags are newer than the lexicon ("H4397" where it has H4397G and H4397H, or the
    // other way round): they take the undivided number's entry, or its first sense.
    let mut added = 0;
    for s in &used {
        if lexicon.contains_key(s) {
            continue;
        }
        let b = original::base(s);
        let found = lexicon
            .get(b)
            .map(|_| b.to_string())
            .or_else(|| lexicon.keys().find(|k| original::base(k) == b).cloned());
        if let Some(k) = found {
            let copy = lexicon[&k].clone();
            lexicon.insert(s.clone(), copy);
            added += 1;
        }
    }
    println!("{added} tags take a related number's lexicon entry");
    let missing: Vec<&String> = used.iter().filter(|s| !lexicon.contains_key(*s)).collect();
    let mut codes: HashSet<String> = HashSet::new();
    for line in verses.values().flatten() {
        codes.insert(line.split('\t').nth(4).unwrap_or_default().to_string());
    }

    let mut out = String::new();
    out.push_str(MAGIC);
    out.push_str("\n#morph\n");
    for (code, text) in &morph {
        out.push_str(&format!("{code}\t{text}\n"));
    }
    out.push_str("#lexicon\n");
    let clean = |s: &str| s.replace(['\t', '\r'], " ").replace('\n', &LINE_BREAK.to_string());
    for (key, l) in lexicon.iter().filter(|(k, _)| used.contains(*k)) {
        out.push_str(&format!(
            "{key}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            clean(&l.lemma),
            clean(&l.translit),
            clean(&l.grammar),
            clean(&l.gloss),
            clean(&l.definition),
            clean(&l.strongs_definition)
        ));
    }
    out.push_str("#words\n");
    for (key, words) in &verses {
        let v = VerseRef::from_key(*key).unwrap();
        out.push_str(&format!("={} {} {}\n", v.book.index(), v.chapter, v.verse));
        for w in words {
            out.push_str(w);
            out.push('\n');
        }
    }

    // NFC, as people and models type: STEPBible writes Greek accents as oxia (U+1F79 ό) where
    // NFC has tonos (U+03CC ό), so a lemma typed in would never match.
    let out: String = out.nfc().collect();

    // Check the result reads back before writing it.
    let decoded = original::Original::parse(out.clone())?;
    let mut unknown_codes: Vec<&String> =
        codes.iter().filter(|c| decoded.describe_grammar(c) == **c && !c.is_empty()).collect();
    unknown_codes.sort();
    let empty: Vec<String> = ChapterIdx::all()
        .flat_map(|c| (1..=c.kjv_verses()).map(move |v| VerseRef::new(c.book(), c.chapter(), v)))
        .filter(|v| !verses.contains_key(&v.key()))
        .map(|v| v.to_string())
        .collect();

    let bytes = zstd::encode_all(out.as_bytes(), 19)?;
    let dir = root.join("data/original");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("original.zst"), &bytes)?;
    println!(
        "{} words in {} verses, {} lexicon entries, {} grammar codes: {:.1} MB text → {:.2} MB",
        decoded.word_count(),
        decoded.verse_count(),
        used.len() - missing.len(),
        morph.len(),
        out.len() as f64 / 1e6,
        bytes.len() as f64 / 1e6
    );
    println!(
        "skipped {} words not in the Hebrew or in neither main Greek text; moved {} words in {} verses past a KJV chapter's end to its last verse",
        stats.skipped,
        stats.clamped_words,
        stats.clamped_verses.len()
    );
    let mut clamped: Vec<&String> = stats.clamped_verses.iter().collect();
    clamped.sort();
    println!("  moved: {}", clamped.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
    println!("KJV verses with no words ({}): {}", empty.len(), empty.join(", "));
    println!("Strong's numbers without a lexicon entry ({}): {:?}", missing.len(), missing);
    println!("grammar codes without a description ({}): {:?}", unknown_codes.len(), unknown_codes);
    let defined = lexicon.iter().filter(|(k, l)| used.contains(*k) && !l.definition.is_empty()).count();
    println!("entries with a definition: {defined} of {}", used.len() - missing.len());
    Ok(())
}

#[derive(Default)]
struct Stats {
    skipped: usize,
    clamped_words: usize,
    clamped_verses: HashSet<String>,
}

#[derive(Default, Clone)]
struct Lex {
    lemma: String,
    translit: String,
    grammar: String,
    gloss: String,
    definition: String,
    strongs_definition: String,
}

/// `code \t parsing` then `\t phrase` on the next line, in each file's FULL MORPHOLOGY CODES.
fn morph_codes(text: &str, out: &mut BTreeMap<String, String>) {
    let Some(start) = text.find("FULL MORPHOLOGY CODES") else { return };
    let lines: Vec<&str> = text[start..].lines().collect();
    for pair in lines.windows(2) {
        let (code, rest) = pair[0].split_once('\t').unwrap_or((pair[0], ""));
        if code.is_empty() || code.contains(' ') || code.starts_with('"') || rest.is_empty() {
            continue;
        }
        let Some(phrase) = pair[1].strip_prefix('\t') else { continue };
        let phrase = phrase.split('\t').next().unwrap_or_default().trim().trim_matches('"').trim();
        if !phrase.is_empty() {
            out.entry(code.to_string())
                .or_insert_with(|| phrase.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
}

/// TBESH/TBESG rows: `eStrong \t dStrong = … \t uStrong \t lemma \t transliteration \t grammar \t
/// gloss \t meaning`, keyed by the dStrong.
fn lexicon_rows(text: &str, out: &mut BTreeMap<String, Lex>, mut define: impl FnMut(&[String], &mut Lex)) {
    for line in text.lines() {
        let row: Vec<String> = line.split('\t').map(|s| s.trim().to_string()).collect();
        let is_row = row[0].len() >= 5
            && (row[0].starts_with('H') || row[0].starts_with('G'))
            && row[0][1..5].bytes().all(|b| b.is_ascii_digit());
        if !is_row || row.len() < 7 {
            continue;
        }
        let Some(d) = row[1].split_whitespace().next().filter(|d| original::normalize_strong(d).is_some())
        else {
            continue;
        };
        if out.contains_key(d) {
            continue;
        }
        let mut lex = Lex {
            lemma: row[3].clone(),
            translit: row[4].clone(),
            grammar: row[5].clone(),
            gloss: row[6].clone(),
            ..Default::default()
        };
        define(&row, &mut lex);
        out.insert(d.to_string(), lex);
    }
}

/// Hebrew definitions from Open Scriptures: the BDB outline entry and Strong's, by eStrong.
struct HebrewDefinitions {
    /// "1254a" → (BDB part of speech, BDB text).
    bdb: HashMap<String, (String, String)>,
    /// "1254" → (Strong's part of speech, Strong's text).
    strong: HashMap<String, (String, String)>,
}

impl HebrewDefinitions {
    fn new(bdb_xml: &str, strong_xml: &str, lexical_index: &str, aug_index: &str) -> Self {
        // AugIndex: augmented Strong's → lexical index id; LexicalIndex: id → BDB entry id.
        let mut aug = HashMap::new();
        let mut current: Option<String> = None;
        for t in xml::tokens(aug_index) {
            match t {
                Tok::Start { name: "w", attrs } => current = xml::attr(attrs, "aug").map(|a| a.into_owned()),
                Tok::Text(s) => {
                    if let Some(a) = current.take() {
                        aug.insert(a, s.trim().to_string());
                    }
                }
                _ => {}
            }
        }
        let mut index_bdb = HashMap::new();
        let mut entry: Option<String> = None;
        for t in xml::tokens(lexical_index) {
            match t {
                Tok::Start { name: "entry", attrs } => entry = xml::attr(attrs, "id").map(|a| a.into_owned()),
                Tok::Empty { name: "xref", attrs } | Tok::Start { name: "xref", attrs } => {
                    if let (Some(id), Some(b)) = (&entry, xml::attr(attrs, "bdb")) {
                        index_bdb.entry(id.clone()).or_insert_with(|| b.into_owned());
                    }
                }
                _ => {}
            }
        }
        let entries = bdb_entries(bdb_xml);
        let bdb = aug
            .into_iter()
            .filter_map(|(a, id)| Some((a, entries.get(index_bdb.get(&id)?)?.clone())))
            .collect();
        Self { bdb, strong: strong_entries(strong_xml) }
    }

    /// Definitions for an eStrong ("H1254A"), unless their part of speech says they're about a
    /// different word: a name where the row isn't one, or the other way round.
    fn get(&self, estrong: &str, grammar: &str) -> Option<(String, String)> {
        let digits: String = estrong[1..].chars().take_while(char::is_ascii_digit).collect();
        let n: u32 = digits.parse().ok()?;
        let letter = estrong[1 + digits.len()..].to_ascii_lowercase();
        let is_name = grammar.starts_with("N:");
        let fits = |pos: &str| pos.contains("pr") == is_name;
        let bdb = self
            .bdb
            .get(&format!("{n}{letter}"))
            .filter(|(pos, _)| fits(pos))
            .map(|(_, t)| format!("BDB: {t}"))
            .unwrap_or_default();
        let strong = self
            .strong
            .get(&n.to_string())
            .filter(|(pos, _)| fits(pos))
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        (!bdb.is_empty() || !strong.is_empty()).then_some((bdb, strong))
    }
}

/// BDB entries by id, as text with each sense on its own line.
fn bdb_entries(xml_text: &str) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    let mut id: Option<String> = None;
    let mut text = String::new();
    let mut pos = String::new();
    let mut in_pos = false;
    let mut skip = 0;
    for t in xml::tokens(xml_text) {
        match t {
            Tok::Start { name: "entry", attrs } => {
                id = xml::attr(attrs, "id").map(|a| a.into_owned());
                text.clear();
                pos.clear();
            }
            Tok::End { name: "entry" } => {
                if let Some(id) = id.take() {
                    out.insert(id, (pos.clone(), drop_count(&tidy(&text))));
                }
            }
            Tok::Start { name: "status", .. } => skip += 1,
            Tok::End { name: "status" } => skip -= 1,
            Tok::Start { name: "sense", attrs } if id.is_some() => {
                text.push('\n');
                if let Some(n) = xml::attr(attrs, "n") {
                    text.push_str(&format!("{n}. "));
                }
            }
            Tok::Start { name: "pos", .. } => in_pos = pos.is_empty(),
            Tok::End { name: "pos" } => in_pos = false,
            Tok::Text(s) if id.is_some() && skip == 0 => {
                if in_pos {
                    pos.push_str(&s);
                }
                text.push_str(&s);
            }
            _ => {}
        }
    }
    out
}

/// The BDB outline follows the headword with how often it occurs ("אָב 1101 n.m. father"),
/// which reads like part of the definition; the word-study tool counts occurrences itself.
fn drop_count(entry: &str) -> String {
    let (first, rest) = entry.split_once('\n').map_or((entry, None), |(a, b)| (a, Some(b)));
    let words: Vec<&str> = first.split(' ').collect();
    let first = match words.get(1) {
        Some(n) if n.bytes().all(|b| b.is_ascii_digit()) => [&words[..1], &words[2..]].concat().join(" "),
        _ => first.to_string(),
    };
    match rest {
        Some(r) => format!("{first}\n{r}"),
        None => first,
    }
}

/// Strong's Hebrew entries by number: its derivation, meaning and the KJV's renderings.
fn strong_entries(xml_text: &str) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    let mut id: Option<String> = None;
    let mut pos = String::new();
    let mut part = "";
    let mut fields: HashMap<&str, String> = HashMap::new();
    for t in xml::tokens(xml_text) {
        match t {
            Tok::Start { name: "entry", attrs } => {
                id = xml::attr(attrs, "id")
                    .and_then(|a| a.strip_prefix('H').map(|n| n.trim_start_matches('0').to_string()));
                fields.clear();
                pos.clear();
            }
            Tok::Start { name: "w", attrs } if part.is_empty() && pos.is_empty() => {
                pos = xml::attr(attrs, "pos").map(|a| a.into_owned()).unwrap_or_default();
            }
            Tok::Start { name: n @ ("source" | "meaning" | "usage"), .. } => part = n,
            Tok::End { name: "source" | "meaning" | "usage" } => part = "",
            Tok::Text(s) if !part.is_empty() => fields.entry(part).or_default().push_str(&s),
            Tok::End { name: "entry" } => {
                if let Some(id) = id.take() {
                    let get = |k| fields.get(k).map(|s| tidy(s)).unwrap_or_default();
                    let mut text = [get("source"), get("meaning")].join(" ").trim().to_string();
                    let usage = get("usage");
                    if !usage.is_empty() {
                        text.push_str(&format!("\nKJV: {usage}"));
                    }
                    out.insert(id, (pos.clone(), text));
                }
            }
            _ => {}
        }
    }
    out
}

/// Collapses runs of spaces within each line and drops empty lines.
fn tidy(s: &str) -> String {
    s.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// STEPBible's lexicon HTML as text: `<br>` starts a line, `__` indents, tags go.
fn html_to_text(s: &str) -> String {
    let s = s.replace("<BR />", "\n").replace("<BR>", "\n").replace("<br>", "\n").replace("<br />", "\n");
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = xml::unescape(&out).replace("__", "");
    tidy(&out)
}

/// "Gen.1.1" → its KJV verse. A verse past the end of a KJV chapter goes to the chapter's last
/// verse; a Psalm title (verse 0) is part of verse 1.
fn kjv_verse(r: &str, stats: &mut Stats) -> Result<(VerseRef, bool)> {
    let mut parts = r.split('.');
    let (Some(b), Some(c), Some(v)) = (parts.next(), parts.next(), parts.next()) else {
        bail!("bad reference {r:?}");
    };
    let book = BOOKS.iter().position(|x| *x == b).with_context(|| format!("unknown book in {r:?}"))?;
    let book = BookId::new(book).unwrap();
    let chapter: u8 = c.parse().with_context(|| format!("bad chapter in {r:?}"))?;
    let verse: u8 = v.parse().with_context(|| format!("bad verse in {r:?}"))?;
    let ch = ChapterIdx::new(book, chapter).with_context(|| format!("no chapter {r:?} in the KJV"))?;
    let title = verse == 0;
    let mut verse = verse.max(1);
    if verse > ch.kjv_verses() {
        stats.clamped_words += 1;
        stats.clamped_verses.insert(r.to_string());
        verse = ch.kjv_verses();
    }
    Ok((VerseRef::new(book, chapter, verse), title))
}

/// TAHOT word lines: `Gen.1.1#01=L \t Hebrew \t transliteration \t English \t dStrongs \t
/// grammar …`. The English reference comes first; the Hebrew one, in brackets, is ignored.
fn hebrew_words(text: &str, verses: &mut BTreeMap<u32, Vec<String>>, stats: &mut Stats) -> Result<()> {
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let Some((r, kind)) = f[0].split_once('#').and_then(|(r, rest)| Some((r, rest.split_once('=')?.1)))
        else {
            continue;
        };
        if f.len() < 6 || !r.contains('.') || r.starts_with(' ') {
            continue;
        }
        // X: words restored from the Septuagint, not in any Hebrew manuscript the KJV used.
        if kind.starts_with('X') || f[1].trim().is_empty() {
            stats.skipped += 1;
            continue;
        }
        let english_ref = r.split('(').next().unwrap();
        let (v, title) = kjv_verse(english_ref, stats)?;
        let strongs = hebrew_strongs(f[4]);
        let word = [
            f[1].replace('\\', ""),
            f[2].to_string(),
            f[3].trim().to_string(),
            strongs,
            f[5].trim().to_string(),
            if title { "t".into() } else { String::new() },
        ];
        verses.entry(v.key()).or_default().push(word.join("\t"));
    }
    Ok(())
}

/// "H9003/{H7225G}\H9016" → "H9003 {H7225G}": prefixes, root and suffixes, without the
/// punctuation after `\`; a lone root loses its braces.
fn hebrew_strongs(s: &str) -> String {
    let mut parts = Vec::new();
    let mut punct = false;
    let mut cur = String::new();
    let flush = |cur: &mut String, punct: bool, parts: &mut Vec<String>| {
        let t = cur.trim().replace('+', "");
        if !t.is_empty() && !punct {
            parts.push(t);
        }
        cur.clear();
    };
    for c in s.chars() {
        match c {
            '/' | '\\' => {
                flush(&mut cur, punct, &mut parts);
                punct = c == '\\';
            }
            _ => cur.push(c),
        }
    }
    flush(&mut cur, punct, &mut parts);
    if parts.len() == 1 {
        parts[0] = parts[0].trim_matches(['{', '}']).to_string();
    }
    parts.join(" ")
}

/// TAGNT word lines: `Mat.1.1#01=NKO \t Greek (transliteration) \t English \t G0976=N-NSF \t
/// lemma=gloss …`. A KJV reference in square brackets wins over the NRSV one.
fn greek_words(text: &str, verses: &mut BTreeMap<u32, Vec<String>>, stats: &mut Stats) -> Result<()> {
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let Some((r, kind)) = f[0].split_once('#').and_then(|(r, rest)| Some((r, rest.split_once('=')?.1)))
        else {
            continue;
        };
        if f.len() < 4 || !r.contains('.') || r.starts_with(' ') {
            continue;
        }
        let (na, tr) = editions(kind);
        if !na && !tr {
            stats.skipped += 1;
            continue;
        }
        let kjv_ref = match (r.find('['), r.find(']')) {
            (Some(a), Some(b)) => format!("{}.{}", r.split('.').next().unwrap(), &r[a + 1..b]),
            _ => r.split(['(', '{', '[']).next().unwrap().to_string(),
        };
        let (v, _) = kjv_verse(&kjv_ref, stats)?;
        let (greek, translit) = match f[1].rsplit_once(" (") {
            Some((g, t)) => (g.trim(), t.trim_end_matches(')').trim()),
            None => (f[1].trim(), ""),
        };
        let mut strongs = Vec::new();
        let mut grammar = Vec::new();
        for part in f[3].split(" + ") {
            let (s, g) = part.split_once('=').unwrap_or((part, ""));
            strongs.push(s.trim());
            grammar.push(g.trim());
        }
        let flags = match (na, tr) {
            (true, false) => "n",
            (false, true) => "k",
            _ => "",
        };
        let word = [greek, translit, f[2].trim(), &strongs.join(" "), &grammar.join(" + "), flags].join("\t");
        verses.entry(v.key()).or_default().push(word);
    }
    Ok(())
}

/// Whether a TAGNT word is in the modern critical text (N) and in the Greek behind the KJV (K).
/// A letter in brackets with a capital is a different word there; a lower-case letter is only
/// a minor difference, so the word counts as present.
fn editions(kind: &str) -> (bool, bool) {
    let mut kept = String::new();
    let mut group = String::new();
    let mut in_group = false;
    for c in kind.chars() {
        match c {
            '(' => in_group = true,
            ')' => {
                if !group.chars().any(|c| c.is_ascii_uppercase()) {
                    kept.push_str(&group);
                }
                group.clear();
                in_group = false;
            }
            _ if in_group => group.push(c),
            _ => kept.push(c),
        }
    }
    (kept.contains(['N', 'n']), kept.contains(['K', 'k']))
}

fn fetch(root: &Path, f: &File) -> Result<PathBuf> {
    let dir = root.join(".sources").join(f.repo.replace('/', "_"));
    std::fs::create_dir_all(&dir)?;
    let name = f.path.rsplit('/').next().unwrap();
    let file = dir.join(name);
    if !file.exists() {
        let encoded: String = f
            .path
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'/' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect();
        let url = format!("https://raw.githubusercontent.com/{}/{}/{encoded}", f.repo, f.commit);
        eprintln!("downloading {url}");
        let status = Command::new("curl").args(["-fsSL", "-o"]).arg(&file).arg(&url).status()?;
        if !status.success() {
            bail!("download failed: {url}");
        }
    }
    let digest: String = Sha256::digest(std::fs::read(&file)?).iter().map(|b| format!("{b:02x}")).collect();
    if digest != f.sha256 {
        bail!("{} sha256 mismatch: expected {}, got {digest}", file.display(), f.sha256);
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hebrew_strongs_keep_prefixes_and_drop_punctuation() {
        assert_eq!(hebrew_strongs("H9003/{H7225G}"), "H9003 {H7225G}");
        assert_eq!(hebrew_strongs("H9009/{H0776G}\\H9016"), "H9009 {H0776G}");
        assert_eq!(hebrew_strongs("{H1254A}"), "H1254A");
        assert_eq!(hebrew_strongs("{H6862A}/H9033\\H9016"), "{H6862A} H9033");
    }

    #[test]
    fn greek_editions() {
        assert_eq!(editions("NKO"), (true, true));
        assert_eq!(editions("N(k)O"), (true, true), "a minor difference");
        assert_eq!(editions("N(K)O"), (true, false), "a different word in the KJV's Greek");
        assert_eq!(editions("K"), (false, true));
        assert_eq!(editions("no"), (true, false));
        assert_eq!(editions("O"), (false, false));
        assert_eq!(editions("(k)O"), (false, true));
    }
}
