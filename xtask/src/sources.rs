//! `cargo xtask build-sources`: the outside sources Ask draws on (docs/plans/m7-outside-sources.md),
//! each read from a public-domain SWORD module pinned by URL and SHA-256 and written to
//! `data/sources/<work>.jsonl.zst`, which `server/scripts/load-sources.ts` loads into Convex. The
//! files are not compiled into the apps.
//!
//! A file's first line is the work (`{"work": {...}}`); every other line is one entry: a
//! commentary's passage (`from`–`to`, verse ids), a dictionary's or topical index's term, or a
//! confession's section, with the plain text and the verses it cites. The output is deterministic,
//! so a rebuild from the same inputs is byte-identical.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use linger_core::books::{BookId, VerseRef};
use linger_core::import::sword::{self, MemFiles, SwordConf};
use linger_core::records::VerseId;
use serde::Serialize;

/// One work: its metadata as `sourceWorks` stores it and the module it comes from.
pub struct Work {
    pub key: &'static str,
    pub title: &'static str,
    pub author: &'static str,
    pub year: u16,
    pub tradition: &'static str,
    pub kind: Kind,
    pub licence: &'static str,
    /// The CrossWire module (`<module>.zip` under `pub/sword/packages/rawzip/`).
    pub module: &'static str,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Commentary,
    Dictionary,
    Topical,
    Confession,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Commentary => "commentary",
            Kind::Dictionary => "dictionary",
            Kind::Topical => "topical",
            Kind::Confession => "confession",
        }
    }
}

const CROSSWIRE: &str = "https://www.crosswire.org/ftpmirror/pub/sword/packages/rawzip";

/// Provenance and the reason each is public domain: `LICENSES/README.md`, "Outside sources".
pub const WORKS: &[Work] = &[
    Work {
        key: "mhc",
        title: "Commentary on the Whole Bible",
        author: "Matthew Henry",
        year: 1710,
        tradition: "Puritan (Presbyterian)",
        kind: Kind::Commentary,
        licence: "Public domain",
        module: "MHC",
        sha256: "6bcb936873ca144e317805e5c1677940fd86e2403f7c14517752e44f25c8882b",
    },
    Work {
        key: "jfb",
        title: "Commentary Critical and Explanatory on the Whole Bible",
        author: "Robert Jamieson, A. R. Fausset and David Brown",
        year: 1871,
        tradition: "Evangelical (Church of Scotland and Anglican)",
        kind: Kind::Commentary,
        licence: "Public domain",
        module: "JFB",
        sha256: "257f743e1c9265618827c236e9b4ccb40ae7a6af348dfe814b555494c4b687fe",
    },
    Work {
        key: "barnes",
        title: "Notes on the New Testament",
        author: "Albert Barnes",
        year: 1834,
        tradition: "Presbyterian (New School)",
        kind: Kind::Commentary,
        licence: "Public domain",
        module: "Barnes",
        sha256: "9e8edc0333875c214124371b3862df118ad3a76ade7753cb2faeed3484615baf",
    },
    Work {
        key: "kd",
        title: "Biblical Commentary on the Old Testament",
        author: "Carl Friedrich Keil and Franz Delitzsch",
        year: 1864,
        tradition: "Lutheran",
        kind: Kind::Commentary,
        licence: "Public domain",
        module: "KD",
        sha256: "ca3bdb6a04c0b717ee2faed71b432d58195db96f217f490ce6f7d9c36a2876f7",
    },
    Work {
        key: "calvin",
        title: "Commentaries",
        author: "John Calvin",
        year: 1555,
        tradition: "Reformed",
        kind: Kind::Commentary,
        licence: "Public domain",
        module: "CalvinCommentaries",
        sha256: "df66fc8c03537499ad006d069481d2c95b600887cdbd6ce75ec5d264b573192a",
    },
    Work {
        key: "easton",
        title: "Illustrated Bible Dictionary",
        author: "M. G. Easton",
        year: 1897,
        tradition: "Presbyterian",
        kind: Kind::Dictionary,
        licence: "Public domain",
        module: "Easton",
        sha256: "f6dd054554764e2e97d5d189a697eb26039054578a9ccf98ce668ab810341c6e",
    },
    Work {
        key: "nave",
        title: "Nave's Topical Bible",
        author: "Orville J. Nave",
        year: 1897,
        tradition: "Methodist",
        kind: Kind::Topical,
        licence: "Public domain",
        module: "Nave",
        sha256: "52d9b7cde04c2abb5187ae804bcb97d93c7344a1358539f50ebc178ac0c945f0",
    },
    Work {
        key: "westminster",
        title: "Westminster Confession of Faith, Larger and Shorter Catechisms",
        author: "Westminster Assembly",
        year: 1647,
        tradition: "Reformed (Presbyterian)",
        kind: Kind::Confession,
        licence: "Public domain",
        module: "Westminster",
        sha256: "54409c931c33a42690d8800efb99e325f7568f9be7332697f314e96618561f99",
    },
    Work {
        key: "lbcf",
        title: "Baptist Confession of Faith",
        author: "Particular Baptist assembly, London",
        year: 1689,
        tradition: "Reformed Baptist",
        kind: Kind::Confession,
        licence: "Public domain",
        module: "BaptistConfession1689",
        sha256: "d6210b1114ea1a6fcd3336524813c08ca3ad57c761814246b15fa278eb8fc98a",
    },
];

pub fn build(root: &Path, keys: &[String]) -> Result<()> {
    let dir = root.join("data/sources");
    std::fs::create_dir_all(&dir)?;
    for work in WORKS.iter().filter(|w| keys.is_empty() || keys.iter().any(|k| k == w.key)) {
        let bytes = build_work(root, work)?;
        std::fs::write(dir.join(format!("{}.jsonl.zst", work.key)), &bytes)?;
    }
    Ok(())
}

/// The work's file, built from its pinned module and printed as a line of counts.
pub fn build_work(root: &Path, work: &Work) -> Result<Vec<u8>> {
    let zip = crate::fetch_pinned(
        root,
        &format!("sword-{}.zip", work.module),
        &format!("{CROSSWIRE}/{}.zip", work.module),
        work.sha256,
    )?;
    let (conf, files) = unzip_module(&zip)?;
    let entries = match work.kind {
        Kind::Commentary => commentary(&conf, &files)?,
        Kind::Dictionary | Kind::Topical => lexicon(&conf, &files)?,
        Kind::Confession => confession(work, &conf, &files)?,
    };
    let mut entries: Vec<Entry> = entries.into_iter().filter(|e| !e.text.is_empty()).collect();
    ensure!(!entries.is_empty(), "{}: no entries", work.key);
    // A key names one entry. A module can hold two under one label (Easton's two KADESH
    // articles; a chapter's heading slot and a run of its verses on the same range): the second
    // is `~2`, in the module's order, so the keys stay stable across rebuilds.
    let mut keys = BTreeSet::new();
    for e in &mut entries {
        let base = e.key.clone();
        for n in 2.. {
            if keys.insert(e.key.clone()) {
                break;
            }
            e.key = format!("{base}~{n}");
        }
    }
    // Fixed-order structs, not `json!` maps: a map's key order follows serde_json's
    // `preserve_order` feature, which other workspace members switch on, and the file must be
    // byte-identical whichever way xtask was built.
    let header = Header {
        work: WorkLine {
            key: work.key,
            title: work.title,
            author: work.author,
            year: work.year,
            tradition: work.tradition,
            kind: work.kind.name(),
            licence: work.licence,
            module: work.module,
            module_version: conf.get("Version").unwrap_or(""),
        },
    };
    let mut text = serde_json::to_string(&header)?;
    text.push('\n');
    let (mut words, mut cites) = (0usize, 0usize);
    for e in &entries {
        words += e.text.split_whitespace().count();
        cites += e.cites.len();
        text.push_str(&serde_json::to_string(&e.line())?);
        text.push('\n');
    }
    let bytes = zstd::encode_all(text.as_bytes(), 19)?;
    println!(
        "{}: {} entries, {:.2} M words, {cites} citations; {:.1} MB → {:.2} MB",
        work.key,
        entries.len(),
        words as f64 / 1e6,
        text.len() as f64 / 1e6,
        bytes.len() as f64 / 1e6
    );
    Ok(bytes)
}

/// One line of a work's file.
struct Entry {
    /// Stable within the work: a passage (`Gen.1.1-5`), a term's key, or a section (`WCF.1.2`).
    key: String,
    /// The passage a commentary entry is on.
    range: Option<(VerseRef, VerseRef)>,
    term: Option<String>,
    heading: Option<String>,
    text: String,
    /// The verses a term or section cites, in order.
    cites: Vec<(VerseRef, VerseRef)>,
}

impl Entry {
    fn line(&self) -> Line<'_> {
        let id = |v: VerseRef| VerseId::new(v.book, v.chapter, v.verse).0;
        Line {
            key: &self.key,
            from: self.range.map(|(from, _)| id(from)),
            to: self.range.map(|(_, to)| id(to)),
            term: self.term.as_deref(),
            heading: self.heading.as_deref(),
            text: &self.text,
            cites: self.cites.iter().map(|&(a, b)| [id(a), id(b)]).collect(),
        }
    }
}

/// The first line of a work's file.
#[derive(Serialize)]
struct Header<'a> {
    work: WorkLine<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkLine<'a> {
    key: &'a str,
    title: &'a str,
    author: &'a str,
    year: u16,
    tradition: &'a str,
    kind: &'a str,
    licence: &'a str,
    module: &'a str,
    module_version: &'a str,
}

/// An entry as its line stores it.
#[derive(Serialize)]
struct Line<'a> {
    key: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    term: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    heading: Option<&'a str>,
    text: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cites: Vec<[u32; 2]>,
}

/// Every file of the module archive in memory, and its one `.conf`.
fn unzip_module(zip: &Path) -> Result<(SwordConf, MemFiles)> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip)?)?;
    let mut files = std::collections::HashMap::new();
    let mut conf = None;
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        if f.is_dir() {
            continue;
        }
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)?;
        let name = f.name().trim_start_matches("./").to_string();
        if name.starts_with("mods.d/") && name.ends_with(".conf") {
            ensure!(conf.is_none(), "{}: more than one .conf", zip.display());
            conf = Some(SwordConf::parse(&String::from_utf8_lossy(&bytes))?);
        }
        files.insert(name, bytes);
    }
    Ok((conf.with_context(|| format!("{}: no mods.d/*.conf", zip.display()))?, MemFiles(files)))
}

/// `Gen.1.1`, `Gen.1.1-5`, `Gen.1.1-2.3`, `Gen.1.1-Exod.2.3`.
fn range_key(from: VerseRef, to: VerseRef) -> String {
    let osis = |b: BookId| b.info().osis;
    if from == to {
        format!("{}.{}.{}", osis(from.book), from.chapter, from.verse)
    } else if from.book != to.book {
        format!(
            "{}.{}.{}-{}.{}.{}",
            osis(from.book),
            from.chapter,
            from.verse,
            osis(to.book),
            to.chapter,
            to.verse
        )
    } else if from.chapter != to.chapter {
        format!("{}.{}.{}-{}.{}", osis(from.book), from.chapter, from.verse, to.chapter, to.verse)
    } else {
        format!("{}.{}.{}-{}", osis(from.book), from.chapter, from.verse, to.verse)
    }
}

fn commentary(conf: &SwordConf, files: &MemFiles) -> Result<Vec<Entry>> {
    Ok(sword::read_commentary(conf, files)?
        .into_iter()
        .map(|e| Entry {
            key: range_key(e.from, e.to),
            range: Some((e.from, e.to)),
            term: None,
            heading: e.heading,
            text: e.text,
            cites: vec![],
        })
        .collect())
}

fn lexicon(conf: &SwordConf, files: &MemFiles) -> Result<Vec<Entry>> {
    Ok(sword::read_lexicon(conf, files)?
        .into_iter()
        .map(|e| Entry {
            key: e.key,
            range: None,
            term: Some(e.name),
            heading: None,
            text: e.text,
            cites: e.cites,
        })
        .collect())
}

/// A confession's sections: the confession's chapters cut into their paragraphs, a catechism's
/// questions, each with its proof texts as the verses it cites.
fn confession(work: &Work, conf: &SwordConf, files: &MemFiles) -> Result<Vec<Entry>> {
    let nodes = sword::read_genbook(conf, files)?;
    let mut out = Vec::new();
    for node in &nodes {
        let (short, chapter) = match (work.key, node.path.first().map(String::as_str), node.path.get(1)) {
            // reformed.org's Westminster: the Confession's chapters, then the catechisms' questions in
            // one node (Shorter) or four (Larger).
            ("westminster", Some("WCF"), Some(ch)) => {
                let n = roman(ch.trim_start_matches("Chapter ").trim())
                    .with_context(|| format!("WCF chapter {ch:?}"))?;
                ("WCF", Some(n))
            }
            ("westminster", Some(short @ ("WSC" | "WLC")), _) => (short, None),
            ("lbcf", Some(ch), None) if ch.starts_with("Chapter ") => {
                let n =
                    ch["Chapter ".len()..].trim().parse().with_context(|| format!("1689 chapter {ch:?}"))?;
                ("LBCF", Some(n))
            }
            _ => continue,
        };
        let (titles, paragraphs) = cut_paragraphs(&node.raw);
        match chapter {
            // A chapter: its title, then paragraphs numbered "I." (Westminster) or titled
            // "Paragraph 1" (1689); a list or title between two paragraphs belongs to the first.
            Some(chapter) => {
                let title =
                    titles.into_iter().filter(|t| !t.starts_with("Chapter ")).collect::<Vec<_>>().join(": ");
                let heading = Some(format!("Chapter {chapter}: {title}"));
                let mut n = 0;
                for raw in paragraphs {
                    let plain = sword::plain_text(conf, &raw);
                    let starts = plain.heading.as_deref().is_some_and(|h| h.starts_with("Paragraph "))
                        || plain.text.split_once(". ").is_some_and(|(num, _)| roman(num).is_some());
                    if starts || out.is_empty() {
                        n += 1;
                        out.push(Entry {
                            key: format!("{short}.{chapter}.{n}"),
                            range: None,
                            term: None,
                            heading: heading.clone(),
                            text: plain.text,
                            cites: plain.cites,
                        });
                    } else {
                        let last: &mut Entry = out.last_mut().unwrap();
                        for t in plain.heading.into_iter().chain([plain.text]).filter(|t| !t.is_empty()) {
                            last.text.push_str("\n\n");
                            last.text.push_str(&t);
                        }
                        last.cites.extend(plain.cites);
                    }
                }
            }
            // A catechism: each paragraph is "Q. 1. The question?" then "A. The answer".
            None => {
                for raw in paragraphs {
                    let plain = sword::plain_text(conf, &raw);
                    let Some(q) = plain.heading.as_deref().and_then(|h| h.strip_prefix("Q. ")) else {
                        continue;
                    };
                    let (number, question) =
                        q.split_once(". ").with_context(|| format!("{short} question {q:?}"))?;
                    let number: u32 =
                        number.trim().parse().with_context(|| format!("{short} question {q:?}"))?;
                    let answer = plain.text.strip_prefix("A. ").unwrap_or(&plain.text).to_string();
                    out.push(Entry {
                        key: format!("{short}.{number}"),
                        range: None,
                        term: None,
                        heading: Some(question.trim().to_string()),
                        text: answer,
                        cites: plain.cites,
                    });
                }
            }
        }
    }
    Ok(out)
}

/// A general book node's markup cut at its top-level `<p>` elements: the plain text of the titles
/// before the first one, then each paragraph's markup with what follows it up to the next (so a
/// list between two paragraphs stays with the first). Notes hold their own `<p>`s, which don't count.
fn cut_paragraphs(raw: &str) -> (Vec<String>, Vec<String>) {
    let mut titles = Vec::new();
    let mut paragraphs: Vec<String> = Vec::new();
    let (mut depth, mut note) = (0usize, 0usize);
    let mut start: Option<usize> = None;
    let mut title_start: Option<usize> = None;
    let mut pos = 0;
    while let Some(lt) = raw[pos..].find('<') {
        let at = pos + lt;
        let Some(gt) = raw[at..].find('>') else { break };
        let tag = &raw[at + 1..at + gt];
        let name =
            tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        let closing = tag.starts_with('/');
        let empty = tag.ends_with('/');
        match name {
            "note" if !empty => {
                if closing {
                    note = note.saturating_sub(1);
                } else {
                    note += 1;
                }
            }
            "p" if note == 0 && !empty => {
                if closing {
                    depth = depth.saturating_sub(1);
                } else {
                    if depth == 0 {
                        if let Some(s) = start {
                            paragraphs.push(raw[s..at].to_string());
                        }
                        start = Some(at);
                    }
                    depth += 1;
                }
            }
            "title" if note == 0 && depth == 0 && start.is_none() && !empty => {
                if closing {
                    if let Some(s) = title_start.take() {
                        titles.push(tidy(&raw[s..at]));
                    }
                } else {
                    title_start = Some(at + gt + 1);
                }
            }
            _ => {}
        }
        pos = at + gt + 1;
    }
    if let Some(s) = start {
        paragraphs.push(raw[s..].to_string());
    }
    (titles, paragraphs)
}

/// A fragment's text without tags, whitespace collapsed.
fn tidy(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A Roman numeral (`XXXIII`) as a number.
fn roman(s: &str) -> Option<u32> {
    let value = |c: char| match c {
        'I' => Some(1),
        'V' => Some(5),
        'X' => Some(10),
        'L' => Some(50),
        'C' => Some(100),
        _ => None,
    };
    let digits: Option<Vec<u32>> = s.chars().map(value).collect();
    let digits = digits?;
    let mut total = 0;
    for (i, d) in digits.iter().enumerate() {
        if digits.get(i + 1).is_some_and(|next| next > d) {
            total -= *d as i64;
        } else {
            total += *d as i64;
        }
    }
    u32::try_from(total).ok().filter(|n| *n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `build-sources` is reproducible: the checked-in file of every work whose pinned module is
    /// in `.sources` is what the pinned module builds today.
    #[test]
    fn checked_in_files_match_the_pinned_inputs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for work in WORKS {
            if !root.join(".sources").join(format!("sword-{}.zip", work.module)).exists() {
                continue;
            }
            let built = build_work(root, work).unwrap();
            let file = root.join("data/sources").join(format!("{}.jsonl.zst", work.key));
            assert!(
                std::fs::read(&file).is_ok_and(|checked_in| checked_in == built),
                "{} differs",
                file.display()
            );
        }
    }

    #[test]
    fn roman_numerals() {
        assert_eq!(roman("I"), Some(1));
        assert_eq!(roman("IV"), Some(4));
        assert_eq!(roman("XIX"), Some(19));
        assert_eq!(roman("XXXIII"), Some(33));
        assert_eq!(roman("Chapter"), None);
    }
}
