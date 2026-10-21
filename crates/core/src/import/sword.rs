//! SWORD modules (CrossWire format): `mods.d/<name>.conf` plus a data directory.
//!
//! Bible text (`read_module`): `zText`/`zText4` (zlib-compressed blocks) and `RawText`/`RawText4`,
//! in KJV, KJVA, NRSV or NRSVA versification, with OSIS, ThML, GBF or plain-text markup. Locked
//! modules (`CipherKey=`) are read with the user's unlock key using SWORD's Sapphire II cipher.
//!
//! Outside sources, for `cargo xtask build-sources` (docs/plans/m7-outside-sources.md):
//! commentaries (`zCom`/`zCom4`, `RawCom`/`RawCom4`; `read_commentary`), dictionaries (`zLD`;
//! `read_lexicon`) and general books (`RawGenBook`; `read_genbook`), whose markup `plain_text`
//! turns into paragraphs and the verses they cite.

use std::collections::HashMap;
use std::io::Read;

use anyhow::{Context, Result, bail, ensure};

use super::osis::OsisHandler;
use crate::books::{BookId, ChapterIdx, FIRST_NT_BOOK, VerseRef};
use crate::model::{MarkKind, Style, TranslationBuilder};
use crate::xml::{Tok, attr, tokens};

/// A parsed `.conf` file.
#[derive(Debug, Clone, Default)]
pub struct SwordConf {
    pub module: String,
    values: HashMap<String, String>,
}

impl SwordConf {
    pub fn parse(text: &str) -> Result<Self> {
        let mut conf = SwordConf::default();
        let mut lines = text.lines();
        for line in lines.by_ref() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                conf.module = name.trim().to_string();
                break;
            }
        }
        ensure!(!conf.module.is_empty(), "not a SWORD .conf file (no [ModuleName] section)");
        let mut pending: Option<(String, String)> = None;
        for line in lines {
            let line = line.trim_end();
            if let Some((k, mut v)) = pending.take() {
                // Continuation of a value that ended in a backslash.
                v.push('\n');
                v.push_str(line.trim_end_matches('\\').trim());
                if line.ends_with('\\') {
                    pending = Some((k, v));
                } else {
                    conf.values.entry(k).or_insert(v);
                }
                continue;
            }
            if line.trim_start().starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let key = k.trim().to_ascii_lowercase();
            if v.ends_with('\\') {
                pending = Some((key, v.trim_end_matches('\\').trim().to_string()));
            } else {
                conf.values.entry(key).or_insert_with(|| v.trim().to_string());
            }
        }
        if let Some((k, v)) = pending {
            conf.values.entry(k).or_insert(v);
        }
        Ok(conf)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(&key.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn description(&self) -> String {
        self.get("Description").map(strip_rtf).unwrap_or_else(|| self.module.clone())
    }

    pub fn copyright(&self) -> String {
        ["ShortCopyright", "Copyright", "DistributionLicense", "ShortPromo"]
            .iter()
            .filter_map(|k| self.get(k))
            .map(strip_rtf)
            .find(|s| !s.is_empty())
            .unwrap_or_default()
    }

    pub fn is_locked(&self) -> bool {
        self.values.contains_key("cipherkey")
    }
}

fn strip_rtf(s: &str) -> String {
    s.replace("\\par", " ").replace("\\pard", " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Read access to the module's files, relative to the module root (the directory holding `mods.d`).
pub trait ModuleFiles {
    fn read(&self, rel: &str) -> Result<Vec<u8>>;
}

pub struct DirFiles(pub std::path::PathBuf);

impl ModuleFiles for DirFiles {
    fn read(&self, rel: &str) -> Result<Vec<u8>> {
        let p = self.0.join(rel.trim_start_matches("./"));
        std::fs::read(&p).with_context(|| format!("reading {}", p.display()))
    }
}

/// Files held in memory (from a zip archive).
pub struct MemFiles(pub HashMap<String, Vec<u8>>);

impl ModuleFiles for MemFiles {
    fn read(&self, rel: &str) -> Result<Vec<u8>> {
        let rel = rel.trim_start_matches("./").to_ascii_lowercase();
        self.0
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase().trim_start_matches("./") == rel)
            .map(|(_, v)| v.clone())
            .with_context(|| format!("module file {rel} not found in archive"))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Driver {
    ZText { size4: bool },
    RawText { size4: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Markup {
    Osis,
    Thml,
    Gbf,
    Plain,
}

/// Reads every canonical verse of the module into the builder.
pub fn read_module(
    b: &mut TranslationBuilder,
    conf: &SwordConf,
    files: &dyn ModuleFiles,
    unlock_key: Option<&str>,
) -> Result<()> {
    let driver = match conf.get("ModDrv").unwrap_or("").to_ascii_lowercase().as_str() {
        "ztext" => Driver::ZText { size4: false },
        "ztext4" => Driver::ZText { size4: true },
        "rawtext" => Driver::RawText { size4: false },
        "rawtext4" => Driver::RawText { size4: true },
        other => bail!("unsupported SWORD driver {other:?} (only Bible text modules are supported)"),
    };
    let v11n = conf.get("Versification").unwrap_or("KJV");
    let verses_in = versification(v11n).with_context(|| {
        format!("SWORD versification {v11n:?} is not supported yet (KJV, KJVA, NRSV and NRSVA are)")
    })?;
    if let Driver::ZText { .. } = driver {
        let c = conf.get("CompressType").unwrap_or("ZIP");
        ensure!(c.eq_ignore_ascii_case("ZIP"), "SWORD compression {c:?} is not supported (ZIP is)");
    }
    let key = match (conf.get("CipherKey").filter(|k| !k.is_empty()), unlock_key.filter(|k| !k.is_empty())) {
        (_, Some(user)) => Some(user.to_string()),
        (Some(k), None) => Some(k.to_string()),
        (None, None) if conf.is_locked() => {
            bail!("this module is locked; enter the unlock key you received from the publisher")
        }
        (None, None) => None,
    };
    let markup = match conf.get("SourceType").unwrap_or("Plaintext").to_ascii_lowercase().as_str() {
        "osis" => Markup::Osis,
        "thml" => Markup::Thml,
        "gbf" => Markup::Gbf,
        _ => Markup::Plain,
    };
    let utf8 = conf.get("Encoding").is_some_and(|e| e.eq_ignore_ascii_case("UTF-8"));
    let data_path = conf.get("DataPath").context("conf has no DataPath")?.trim_end_matches('/').to_string();

    let mut cipher_check: Option<usize> = None;
    let block = block_prefix(conf);
    for (testament, books) in [("ot", 0..FIRST_NT_BOOK.0), ("nt", FIRST_NT_BOOK.0..66)] {
        let reader = match TestamentReader::open(files, &data_path, testament, driver, key.as_deref(), block)
        {
            Ok(r) => r,
            // A module may cover only one testament.
            Err(_) => continue,
        };
        // Index layout: [0] module heading, [1] testament heading, then per book a heading entry
        // and per chapter a heading entry followed by its verses (so Genesis 1:1 is entry 4).
        let mut idx = 1usize;
        let mut handler = OsisHandler::new(b, true);
        for book in books.map(BookId) {
            idx += 1;
            handler.b.begin_book(book);
            for ch in 1..=book.chapters() {
                idx += 1;
                let c = ChapterIdx::new(book, ch).unwrap();
                handler.b.begin_chapter(ch as u32);
                let n = verses_in(c);
                for v in 1..=n {
                    let raw = reader.entry(idx + v as usize)?;
                    if raw.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let text = if utf8 {
                        String::from_utf8_lossy(&raw).into_owned()
                    } else {
                        raw.iter().map(|&b| b as char).collect()
                    };
                    if key.is_some() && cipher_check.is_none() {
                        cipher_check = Some(0);
                        // A wrong key produces binary garbage; refuse rather than import noise.
                        let printable = text.chars().filter(|c| !c.is_control() || c.is_whitespace()).count();
                        ensure!(
                            printable * 10 >= text.chars().count() * 9,
                            "the unlock key is not correct for this module"
                        );
                    }
                    handler.b.begin_verse(v as u32, v as u32);
                    match markup {
                        Markup::Osis => {
                            handler.feed(&text);
                            handler.end_fragment();
                        }
                        Markup::Thml => thml(handler.b, &text),
                        Markup::Gbf => gbf(handler.b, &text),
                        Markup::Plain => handler.b.text(&text),
                    }
                }
                idx += n as usize;
            }
        }
    }
    Ok(())
}

/// What a commentary says on the verses `from`–`to`: one entry per run of verse slots the module
/// links to the same text, in canon order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentaryEntry {
    pub from: VerseRef,
    pub to: VerseRef,
    pub heading: Option<String>,
    pub text: String,
}

/// A dictionary's or topical index's entry: its key as the module sorts it, its name as written,
/// its text, and the verses it cites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexiconEntry {
    pub key: String,
    pub name: String,
    pub text: String,
    pub cites: Vec<(VerseRef, VerseRef)>,
}

/// A general book's node, in tree order: its path of names from the root and its markup (the
/// `SourceType` of the conf), which `plain_text` reads once the caller has cut it into sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenBookNode {
    pub path: Vec<String>,
    pub raw: String,
}

/// A fragment of markup as plain text: its first title (if it opens with one), its paragraphs
/// separated by blank lines, and the verses its references cite, in order of citation. Notes
/// (footnotes, which hold a confession's proof-text quotations) keep their citations and lose
/// their text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlainText {
    pub heading: Option<String>,
    pub text: String,
    pub cites: Vec<(VerseRef, VerseRef)>,
}

/// Every entry of a commentary module (`zCom`, `zCom4`, `RawCom`, `RawCom4`), in canon order.
/// Consecutive slots whose index records are identical are one entry (SWORD's linked entries);
/// a book's or chapter's heading slot is an entry on the whole book or chapter.
pub fn read_commentary(conf: &SwordConf, files: &dyn ModuleFiles) -> Result<Vec<CommentaryEntry>> {
    let driver = match conf.get("ModDrv").unwrap_or("").to_ascii_lowercase().as_str() {
        "zcom" => Driver::ZText { size4: false },
        "zcom4" => Driver::ZText { size4: true },
        "rawcom" => Driver::RawText { size4: false },
        "rawcom4" => Driver::RawText { size4: true },
        other => bail!("unsupported SWORD driver {other:?} (a commentary is zCom, zCom4, RawCom or RawCom4)"),
    };
    let v11n = conf.get("Versification").unwrap_or("KJV");
    let verses_in =
        versification(v11n).with_context(|| format!("SWORD versification {v11n:?} is not supported"))?;
    let data_path = conf.get("DataPath").context("conf has no DataPath")?.trim_end_matches('/').to_string();
    let markup = markup_of(conf);
    let utf8 = conf.get("Encoding").is_some_and(|e| e.eq_ignore_ascii_case("UTF-8"));
    let block = block_prefix(conf);

    let mut out = Vec::new();
    for (testament, books) in [("ot", 0..FIRST_NT_BOOK.0), ("nt", FIRST_NT_BOOK.0..66)] {
        let Ok(reader) = TestamentReader::open(files, &data_path, testament, driver, None, block) else {
            continue;
        };
        // The slots in index order, each with the range it stands for (none for the module's and
        // the testament's own headings).
        let mut slots: Vec<Option<(VerseRef, VerseRef)>> = vec![None, None];
        for book in books.map(BookId) {
            let last = ChapterIdx::new(book, book.chapters()).unwrap();
            slots.push(Some((
                VerseRef::new(book, 1, 1),
                VerseRef::new(book, book.chapters(), verses_in(last)),
            )));
            for ch in 1..=book.chapters() {
                let n = verses_in(ChapterIdx::new(book, ch).unwrap());
                slots.push(Some((VerseRef::new(book, ch, 1), VerseRef::new(book, ch, n))));
                for v in 1..=n {
                    slots.push(Some((VerseRef::new(book, ch, v), VerseRef::new(book, ch, v))));
                }
            }
        }
        let mut idx = 0;
        while idx < slots.len() {
            let record = reader.record(idx);
            let mut end = idx + 1;
            while end < slots.len() && reader.record(end) == record {
                end += 1;
            }
            let covered: Vec<(VerseRef, VerseRef)> = slots[idx..end].iter().flatten().copied().collect();
            if let (Some(record), Some(first)) = (record, covered.first())
                && !record.is_empty()
            {
                let raw = reader.entry(idx)?;
                let text = decode(&raw, utf8);
                let plain = plain_text_of(markup, &text);
                if !plain.text.is_empty() || plain.heading.is_some() {
                    let from = covered.iter().map(|r| r.0).min().unwrap_or(first.0);
                    let to = covered.iter().map(|r| r.1).max().unwrap_or(first.1);
                    out.push(CommentaryEntry { from, to, heading: plain.heading, text: plain.text });
                }
            }
            idx = end;
        }
    }
    Ok(out)
}

/// Every entry of a `zLD` dictionary module, in the module's key order.
pub fn read_lexicon(conf: &SwordConf, files: &dyn ModuleFiles) -> Result<Vec<LexiconEntry>> {
    let drv = conf.get("ModDrv").unwrap_or("");
    ensure!(drv.eq_ignore_ascii_case("zLD"), "unsupported SWORD driver {drv:?} (a dictionary is zLD)");
    let path = conf.get("DataPath").context("conf has no DataPath")?.trim_end_matches('/');
    let markup = markup_of(conf);
    let utf8 = conf.get("Encoding").is_some_and(|e| e.eq_ignore_ascii_case("UTF-8"));
    let idx = files.read(&format!("{path}.idx"))?;
    let dat = files.read(&format!("{path}.dat"))?;
    let zdx = files.read(&format!("{path}.zdx"))?;
    let zdt = files.read(&format!("{path}.zdt"))?;
    let u32_at = |b: &[u8], at: usize| -> Result<usize> {
        Ok(u32::from_le_bytes(b.get(at..at + 4).context("short record")?.try_into().unwrap()) as usize)
    };
    let mut blocks: HashMap<usize, Vec<u8>> = HashMap::new();
    let mut out = Vec::new();
    for rec in idx.chunks_exact(8) {
        let (off, size) = (u32_at(rec, 0)?, u32_at(rec, 4)?);
        let dat_rec = dat.get(off..off + size).context("dictionary key out of range")?;
        let nl = dat_rec.iter().position(|&b| b == b'\n').context("dictionary key without a newline")?;
        let key = decode(dat_rec[..nl].strip_suffix(b"\r").unwrap_or(&dat_rec[..nl]), utf8);
        let (block, entry) = (u32_at(dat_rec, nl + 1)?, u32_at(dat_rec, nl + 5)?);
        if let std::collections::hash_map::Entry::Vacant(e) = blocks.entry(block) {
            let (boff, bsize) = (u32_at(&zdx, block * 8)?, u32_at(&zdx, block * 8 + 4)?);
            let comp = zdt.get(boff..boff + bsize).context("dictionary block out of range")?;
            let mut plain = Vec::new();
            flate2::read::ZlibDecoder::new(comp)
                .read_to_end(&mut plain)
                .context("decompressing a dictionary block")?;
            e.insert(plain);
        }
        let data = &blocks[&block];
        let count = u32_at(data, 0)?;
        ensure!(entry < count, "dictionary entry {entry} past the block's {count}");
        let (eoff, esize) = (u32_at(data, 4 + entry * 8)?, u32_at(data, 8 + entry * 8)?);
        let raw = data.get(eoff..eoff + esize).context("dictionary entry out of range")?;
        let raw = raw.strip_suffix(b"\0").unwrap_or(raw);
        let text = decode(raw, utf8);
        let name = tokens(&text)
            .find_map(|t| match t {
                Tok::Start { name, attrs } if name.eq_ignore_ascii_case("entryFree") => {
                    attr(attrs, "n").map(|n| n.into_owned())
                }
                _ => None,
            })
            .unwrap_or_else(|| key.clone());
        let plain = plain_text_of(markup, &text);
        out.push(LexiconEntry { key, name, text: plain.text, cites: plain.cites });
    }
    Ok(out)
}

/// Every node of a `RawGenBook` module with text, depth first in the module's order.
pub fn read_genbook(conf: &SwordConf, files: &dyn ModuleFiles) -> Result<Vec<GenBookNode>> {
    let drv = conf.get("ModDrv").unwrap_or("");
    ensure!(
        drv.eq_ignore_ascii_case("RawGenBook"),
        "unsupported SWORD driver {drv:?} (a general book is RawGenBook)"
    );
    let path = conf.get("DataPath").context("conf has no DataPath")?.trim_end_matches('/');
    let utf8 = conf.get("Encoding").is_some_and(|e| e.eq_ignore_ascii_case("UTF-8"));
    let idx = files.read(&format!("{path}.idx"))?;
    let dat = files.read(&format!("{path}.dat"))?;
    let bdt = files.read(&format!("{path}.bdt"))?;
    let u32_at = |b: &[u8], at: usize| -> Result<usize> {
        Ok(u32::from_le_bytes(b.get(at..at + 4).context("short record")?.try_into().unwrap()) as usize)
    };
    const NONE: usize = u32::MAX as usize;
    // A node's links are offsets into `.idx`, each holding the node's offset into `.dat`.
    let mut out = Vec::new();
    let mut stack: Vec<(usize, Vec<String>)> = vec![(0, Vec::new())];
    let mut seen = 0usize;
    while let Some((at, path)) = stack.pop() {
        seen += 1;
        ensure!(seen <= idx.len() / 4 + 1, "the book's tree loops");
        let off = u32_at(&idx, at)?;
        let (next, child) = (u32_at(&dat, off + 4)?, u32_at(&dat, off + 8)?);
        let name_end =
            off + 12 + dat[off + 12..].iter().position(|&b| b == 0).context("node name without a NUL")?;
        let name = decode(&dat[off + 12..name_end], utf8);
        let user_size = u16::from_le_bytes(
            dat.get(name_end + 1..name_end + 3).context("short node")?.try_into().unwrap(),
        ) as usize;
        let mut node_path = path.clone();
        if at != 0 {
            node_path.push(name);
        }
        if user_size == 8 {
            let (toff, tsize) = (u32_at(&dat, name_end + 3)?, u32_at(&dat, name_end + 7)?);
            let raw = bdt.get(toff..toff + tsize).context("node text out of range")?;
            let raw = raw.strip_suffix(b"\0").unwrap_or(raw);
            if !raw.iter().all(u8::is_ascii_whitespace) {
                out.push(GenBookNode { path: node_path.clone(), raw: decode(raw, utf8) });
            }
        }
        // Siblings after children: the stack pops the child first.
        if next != NONE && at != 0 {
            stack.push((next, path));
        }
        if child != NONE {
            stack.push((child, node_path));
        }
    }
    Ok(out)
}

/// `src`, a fragment in the conf's `SourceType` markup, as plain text with its citations.
pub fn plain_text(conf: &SwordConf, src: &str) -> PlainText {
    plain_text_of(markup_of(conf), src)
}

fn markup_of(conf: &SwordConf) -> Markup {
    match conf.get("SourceType").unwrap_or("Plaintext").to_ascii_lowercase().as_str() {
        "osis" | "tei" => Markup::Osis,
        "thml" => Markup::Thml,
        "gbf" => Markup::Gbf,
        _ => Markup::Plain,
    }
}

/// The letter before `zs`/`zv`/`zz` in a compressed module's file names: `b` when a block holds
/// a book (the default), `c` a chapter, `v` a verse.
fn block_prefix(conf: &SwordConf) -> char {
    match conf.get("BlockType").unwrap_or("BOOK").to_ascii_uppercase().as_str() {
        "CHAPTER" => 'c',
        "VERSE" => 'v',
        _ => 'b',
    }
}

fn decode(raw: &[u8], utf8: bool) -> String {
    if utf8 { String::from_utf8_lossy(raw).into_owned() } else { raw.iter().map(|&b| b as char).collect() }
}

fn plain_text_of(markup: Markup, src: &str) -> PlainText {
    let mut out = PlainText::default();
    if markup == Markup::Gbf || markup == Markup::Plain {
        out.text = tidy_blocks(&src.replace("<CM>", "\n\n").replace("<CL>", "\n"));
        return out;
    }
    let mut blocks: Vec<(bool, String)> = Vec::new();
    let mut cur = String::new();
    let mut title_depth = 0usize;
    let mut skip = 0usize;
    let flush = |cur: &mut String, blocks: &mut Vec<(bool, String)>, title: bool| {
        let text = tidy_block(cur);
        cur.clear();
        if !text.is_empty() {
            blocks.push((title, text));
        }
    };
    for tok in tokens(src) {
        match tok {
            Tok::Text(t) => {
                if skip == 0 {
                    cur.push_str(&t);
                }
            }
            Tok::Start { name, attrs } | Tok::Empty { name, attrs } => {
                let n = name.to_ascii_lowercase();
                let empty = matches!(tok, Tok::Empty { .. });
                match n.as_str() {
                    "reference" | "ref" | "scripref" => {
                        if let Some(r) = attr(attrs, "osisRef") {
                            for range in r.split_whitespace().filter_map(osis_range) {
                                if out.cites.last() != Some(&range) {
                                    out.cites.push(range);
                                }
                            }
                        }
                    }
                    "note" if !empty => skip += 1,
                    "title" | "head" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if skip == 0 && !empty => {
                        flush(&mut cur, &mut blocks, title_depth > 0);
                        title_depth += 1;
                    }
                    "p" | "div" | "lg" | "list" | "item" | "table" | "tr" | "entryfree" | "entry" | "def"
                        if skip == 0 && title_depth == 0 =>
                    {
                        flush(&mut cur, &mut blocks, false)
                    }
                    "l" | "lb" | "br" if skip == 0 => cur.push('\n'),
                    // OSIS marks a paragraph break inside verse-per-line text with a milestone.
                    "milestone"
                        if skip == 0 && attr(attrs, "type").is_some_and(|t| t == "x-p" || t == "line") =>
                    {
                        cur.push_str("\n\n")
                    }
                    _ => {}
                }
            }
            Tok::End { name } => {
                let n = name.to_ascii_lowercase();
                match n.as_str() {
                    "note" => skip = skip.saturating_sub(1),
                    "title" | "head" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                        if skip == 0 && title_depth > 0 =>
                    {
                        title_depth -= 1;
                        if title_depth == 0 {
                            flush(&mut cur, &mut blocks, true);
                        }
                    }
                    "p" | "div" | "lg" | "list" | "item" | "table" | "tr" | "entryfree" | "entry" | "def"
                        if skip == 0 && title_depth == 0 =>
                    {
                        flush(&mut cur, &mut blocks, false)
                    }
                    "l" if skip == 0 => cur.push('\n'),
                    _ => {}
                }
            }
        }
    }
    flush(&mut cur, &mut blocks, title_depth > 0);
    let mut blocks = blocks.into_iter();
    let mut first = blocks.next();
    if let Some((true, title)) = &first {
        out.heading = Some(title.clone());
        first = blocks.next();
    }
    out.text = first.into_iter().chain(blocks).map(|(_, b)| b).collect::<Vec<_>>().join("\n\n");
    out
}

/// One block's text: each line's whitespace collapsed, empty lines dropped.
fn tidy_block(s: &str) -> String {
    s.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Plain text: blank lines separate paragraphs, other whitespace collapses.
fn tidy_blocks(s: &str) -> String {
    s.split("\n\n").map(tidy_block).filter(|b| !b.is_empty()).collect::<Vec<_>>().join("\n\n")
}

/// One osisRef (`Bible:Exod.6.20`, `Exod.6.16-Exod.6.20`, `Matt.22`, `Gen`) as a range on the KJV
/// grid: a chapter or book is all its verses, a verse past a chapter's end is its last, a verse 0
/// (a Psalm's title) is verse 1. `None` for a book Linger doesn't have.
pub fn osis_range(r: &str) -> Option<(VerseRef, VerseRef)> {
    let r = r.rsplit_once(':').map_or(r, |(_, r)| r);
    let (a, b) = r.split_once('-').unwrap_or((r, r));
    // "Ps.3.0" names the title, which Linger folds into verse 1; "Ps.3" the chapter.
    let parse = |s: &str| {
        let (book, c, v) = super::osis::parse_osis_ref(s)?;
        Some((book, c, if v == 0 && s.ends_with(".0") { 1 } else { v }))
    };
    let (book, c1, v1) = parse(a)?;
    let (book2, c2, v2) = if b == a {
        (book, c1, v1)
    } else if b.contains('.') && BookId::from_osis(b.split('.').next()?).is_some() {
        parse(b)?
    } else {
        // A short end: "Gen.1.1-5" or "Gen.1.1-2.3".
        let mut nums = b.split('.').filter_map(|p| p.parse::<u32>().ok());
        match (nums.next(), nums.next()) {
            (Some(x), Some(y)) => (book, x, y),
            (Some(x), None) if c1 > 0 && v1 > 0 => (book, c1, x),
            (Some(x), None) => (book, x, 0),
            _ => (book, c1, v1),
        }
    };
    let clamp = |book: BookId, c: u32, v: u32, end: bool| -> Option<VerseRef> {
        let c = if c == 0 { if end { book.chapters() as u32 } else { 1 } } else { c };
        let idx = ChapterIdx::new(book, u8::try_from(c).ok()?)?;
        let last = idx.kjv_verses();
        let v = if v == 0 { if end { last as u32 } else { 1 } } else { v.min(last as u32) };
        Some(VerseRef::new(book, c as u8, v as u8))
    };
    let from = clamp(book, c1, v1, false)?;
    let to = clamp(book2, c2, v2, true)?;
    Some(if to < from { (from, from) } else { (from, to) })
}

/// Verses per chapter for a SWORD versification whose 66 canonical books are in KJV order.
/// (KJVA and NRSVA add the Apocrypha after Malachi, which doesn't move any canonical entry.)
/// Tables from SWORD's `canon.h` / `canon_nrsv.h`.
fn versification(name: &str) -> Option<fn(ChapterIdx) -> u8> {
    fn kjv(c: ChapterIdx) -> u8 {
        c.kjv_verses()
    }
    fn nrsv(c: ChapterIdx) -> u8 {
        // 3 John 1 and Revelation 12 each have one more verse than in the KJV.
        match c.0 {
            1165 => 15,
            1178 => 18,
            _ => c.kjv_verses(),
        }
    }
    match name.to_ascii_uppercase().as_str() {
        "KJV" | "KJVA" => Some(kjv),
        "NRSV" | "NRSVA" => Some(nrsv),
        _ => None,
    }
}

struct TestamentReader {
    driver: Driver,
    index: Vec<u8>,
    data: Vec<u8>,
    blocks: Vec<u8>,
    key: Option<String>,
    cache: std::cell::RefCell<Option<(usize, std::rc::Rc<Vec<u8>>)>>,
}

impl TestamentReader {
    fn open(
        files: &dyn ModuleFiles,
        dir: &str,
        t: &str,
        driver: Driver,
        key: Option<&str>,
        block: char,
    ) -> Result<Self> {
        let (index, data, blocks) = match driver {
            Driver::ZText { .. } => (
                files.read(&format!("{dir}/{t}.{block}zv"))?,
                files.read(&format!("{dir}/{t}.{block}zz"))?,
                files.read(&format!("{dir}/{t}.{block}zs"))?,
            ),
            Driver::RawText { .. } => {
                (files.read(&format!("{dir}/{t}.vss"))?, files.read(&format!("{dir}/{t}"))?, Vec::new())
            }
        };
        Ok(Self { driver, index, data, blocks, key: key.map(str::to_string), cache: Default::default() })
    }

    /// The slot's index record, which names its bytes in the data: two slots with the same
    /// record share one text (a commentary entry on a range). `None` past the index's end.
    fn record(&self, idx: usize) -> Option<&[u8]> {
        let w = match self.driver {
            Driver::RawText { size4 } => {
                if size4 {
                    8
                } else {
                    6
                }
            }
            Driver::ZText { size4 } => {
                if size4 {
                    12
                } else {
                    10
                }
            }
        };
        let rec = self.index.get(idx * w..idx * w + w)?;
        // A record whose length is 0 names nothing.
        let len_zero = match self.driver {
            Driver::RawText { size4: false } => rec[4..6] == [0, 0],
            Driver::RawText { size4: true } => rec[4..8] == [0; 4],
            Driver::ZText { size4: false } => rec[8..10] == [0, 0],
            Driver::ZText { size4: true } => rec[8..12] == [0; 4],
        };
        Some(if len_zero { &[] } else { rec })
    }

    fn entry(&self, idx: usize) -> Result<Vec<u8>> {
        match self.driver {
            Driver::RawText { size4 } => {
                let w = if size4 { 8 } else { 6 };
                let Some(rec) = self.index.get(idx * w..idx * w + w) else { return Ok(Vec::new()) };
                let off = u32::from_le_bytes(rec[0..4].try_into().unwrap()) as usize;
                let len = if size4 {
                    u32::from_le_bytes(rec[4..8].try_into().unwrap()) as usize
                } else {
                    u16::from_le_bytes(rec[4..6].try_into().unwrap()) as usize
                };
                let mut raw = self.data.get(off..off + len).context("verse out of range")?.to_vec();
                if let Some(k) = &self.key {
                    Sapphire::new(k.as_bytes()).decrypt_in_place(&mut raw);
                }
                Ok(raw)
            }
            Driver::ZText { size4 } => {
                let w = if size4 { 12 } else { 10 };
                let Some(rec) = self.index.get(idx * w..idx * w + w) else { return Ok(Vec::new()) };
                let block = u32::from_le_bytes(rec[0..4].try_into().unwrap()) as usize;
                let off = u32::from_le_bytes(rec[4..8].try_into().unwrap()) as usize;
                let len = if size4 {
                    u32::from_le_bytes(rec[8..12].try_into().unwrap()) as usize
                } else {
                    u16::from_le_bytes(rec[8..10].try_into().unwrap()) as usize
                };
                if len == 0 {
                    return Ok(Vec::new());
                }
                let data = self.block(block)?;
                Ok(data.get(off..off + len).context("verse outside its block")?.to_vec())
            }
        }
    }

    fn block(&self, n: usize) -> Result<std::rc::Rc<Vec<u8>>> {
        if let Some((i, d)) = &*self.cache.borrow()
            && *i == n
        {
            return Ok(d.clone());
        }
        let rec = self.blocks.get(n * 12..n * 12 + 12).context("block index out of range")?;
        let off = u32::from_le_bytes(rec[0..4].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(rec[4..8].try_into().unwrap()) as usize;
        let ucsize = u32::from_le_bytes(rec[8..12].try_into().unwrap()) as usize;
        let mut comp = self.data.get(off..off + size).context("block out of range")?.to_vec();
        if let Some(k) = &self.key {
            Sapphire::new(k.as_bytes()).decrypt_in_place(&mut comp);
        }
        let mut out = Vec::with_capacity(ucsize);
        flate2::read::ZlibDecoder::new(&comp[..])
            .read_to_end(&mut out)
            .context("could not decompress a module block (wrong unlock key?)")?;
        let rc = std::rc::Rc::new(out);
        *self.cache.borrow_mut() = Some((n, rc.clone()));
        Ok(rc)
    }
}

/// ThML markup (older CrossWire modules).
fn thml(b: &mut TranslationBuilder, src: &str) {
    let mut skip = 0usize;
    let mut styles: Vec<(String, Option<Style>)> = Vec::new();
    let mut heading: Option<String> = None;
    for tok in tokens(src) {
        match tok {
            Tok::Text(t) if skip == 0 => match &mut heading {
                Some(h) => h.push_str(&t),
                None => b.text(&t),
            },
            Tok::Text(_) => {}
            Tok::Start { name, attrs } => {
                let n = name.to_ascii_lowercase();
                if skip > 0 || matches!(n.as_str(), "note" | "scripref" | "scripture") {
                    skip += 1;
                    continue;
                }
                let class = attr(attrs, "class").unwrap_or_default().to_ascii_lowercase();
                let color = attr(attrs, "color").unwrap_or_default().to_ascii_lowercase();
                let style = match n.as_str() {
                    "font" if color.contains("red") || color.contains("#ff0000") => Some(Style::WordsOfJesus),
                    "span" if class.contains("jesus") || class.contains("wj") => Some(Style::WordsOfJesus),
                    "span" | "added" if class.contains("added") || n == "added" => Some(Style::Added),
                    "i" | "em" => Some(Style::Italic),
                    "b" | "strong" => Some(Style::Bold),
                    _ => None,
                };
                if n == "p" {
                    b.mark(MarkKind::Paragraph, 0);
                } else if matches!(n.as_str(), "h1" | "h2" | "h3" | "h4") || class.contains("sechead") {
                    heading = Some(String::new());
                }
                if let Some(s) = style {
                    b.push_style(s);
                }
                styles.push((n, style));
            }
            Tok::Empty { name, .. } => {
                if skip == 0 && name.eq_ignore_ascii_case("br") {
                    b.mark(MarkKind::Poetry, 1);
                }
            }
            Tok::End { name } => {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let n = name.to_ascii_lowercase();
                if let Some(i) = styles.iter().rposition(|(s, _)| *s == n)
                    && let (_, Some(s)) = styles.remove(i)
                {
                    b.pop_style(s);
                }
                if let (Some(h), true) = (&heading, matches!(n.as_str(), "h1" | "h2" | "h3" | "h4" | "div")) {
                    let h = h.clone();
                    b.heading(MarkKind::Heading, 1, &h);
                    heading = None;
                }
            }
        }
    }
    b.clear_styles();
}

/// GBF (General Bible Format) markup: `<FR>…<Fr>`, `<RF>note<Rf>`, `<CM>`, `<TS>title<Ts>`, …
fn gbf(b: &mut TranslationBuilder, src: &str) {
    let mut rest = src;
    let mut skip = false;
    let mut title: Option<String> = None;
    while !rest.is_empty() {
        let (text, tag, next) = match rest.find('<') {
            Some(i) => match rest[i..].find('>') {
                Some(j) => (&rest[..i], Some(&rest[i + 1..i + j]), &rest[i + j + 1..]),
                None => (rest, None, ""),
            },
            None => (rest, None, ""),
        };
        if !skip {
            match &mut title {
                Some(t) => t.push_str(text),
                None => b.text(text),
            }
        }
        rest = next;
        let Some(tag) = tag else { continue };
        match tag {
            "RF" | "RX" => skip = true,
            "Rf" | "Rx" => skip = false,
            "FR" => b.push_style(Style::WordsOfJesus),
            "Fr" => b.pop_style(Style::WordsOfJesus),
            "FI" => b.push_style(Style::Italic),
            "Fi" => b.pop_style(Style::Italic),
            "FB" => b.push_style(Style::Bold),
            "Fb" => b.pop_style(Style::Bold),
            "FS" => b.push_style(Style::SmallCaps),
            "Fs" => b.pop_style(Style::SmallCaps),
            "CM" => b.mark(MarkKind::Paragraph, 0),
            "CL" => b.mark(MarkKind::Poetry, 1),
            "TS" => title = Some(String::new()),
            "Ts" => {
                if let Some(t) = title.take() {
                    b.heading(MarkKind::Heading, 1, &t);
                }
            }
            _ => {}
        }
    }
    b.clear_styles();
}

/// Sapphire II stream cipher (Michael Paul Johnson), as used by SWORD for locked modules.
pub struct Sapphire {
    cards: [u8; 256],
    rotor: u8,
    ratchet: u8,
    avalanche: u8,
    last_plain: u8,
    last_cipher: u8,
}

impl Sapphire {
    pub fn new(key: &[u8]) -> Self {
        let mut s =
            Sapphire { cards: [0; 256], rotor: 0, ratchet: 0, avalanche: 0, last_plain: 0, last_cipher: 0 };
        for (i, c) in s.cards.iter_mut().enumerate() {
            *c = i as u8;
        }
        if key.is_empty() {
            // SWORD never calls this without a key; match hash_init for completeness.
            s.rotor = 1;
            s.ratchet = 3;
            s.avalanche = 5;
            s.last_plain = 7;
            s.last_cipher = 11;
            for i in 0..256 {
                s.cards[i] = (255 - i) as u8;
            }
            return s;
        }
        let mut rsum: u8 = 0;
        let mut keypos: usize = 0;
        for i in (0..=255usize).rev() {
            let toswap = s.keyrand(i as u32, key, &mut rsum, &mut keypos) as usize;
            s.cards.swap(i, toswap);
        }
        s.rotor = s.cards[1];
        s.ratchet = s.cards[3];
        s.avalanche = s.cards[5];
        s.last_plain = s.cards[7];
        s.last_cipher = s.cards[rsum as usize];
        s
    }

    fn keyrand(&self, limit: u32, key: &[u8], rsum: &mut u8, keypos: &mut usize) -> u8 {
        if limit == 0 {
            return 0;
        }
        let mut retry = 0u32;
        let mut mask: u32 = 1;
        while mask < limit {
            mask = (mask << 1) + 1;
        }
        loop {
            *rsum = self.cards[*rsum as usize].wrapping_add(key[*keypos]);
            *keypos += 1;
            if *keypos >= key.len() {
                *keypos = 0;
                *rsum = rsum.wrapping_add(key.len() as u8);
            }
            let mut u = mask & *rsum as u32;
            retry += 1;
            if retry > 11 {
                u %= limit;
            }
            if u <= limit {
                return u as u8;
            }
        }
    }

    fn step(&mut self) -> u8 {
        self.ratchet = self.ratchet.wrapping_add(self.cards[self.rotor as usize]);
        self.rotor = self.rotor.wrapping_add(1);
        let swaptemp = self.cards[self.last_cipher as usize];
        self.cards[self.last_cipher as usize] = self.cards[self.ratchet as usize];
        self.cards[self.ratchet as usize] = self.cards[self.last_plain as usize];
        self.cards[self.last_plain as usize] = self.cards[self.rotor as usize];
        self.cards[self.rotor as usize] = swaptemp;
        self.avalanche = self.avalanche.wrapping_add(self.cards[swaptemp as usize]);
        let c = &self.cards;
        let a = c[(c[self.ratchet as usize].wrapping_add(c[self.rotor as usize])) as usize];
        let i = c[self.last_plain as usize]
            .wrapping_add(c[self.last_cipher as usize])
            .wrapping_add(c[self.avalanche as usize]);
        a ^ c[c[i as usize] as usize]
    }

    pub fn decrypt(&mut self, b: u8) -> u8 {
        let k = self.step();
        self.last_plain = b ^ k;
        self.last_cipher = b;
        self.last_plain
    }

    pub fn encrypt(&mut self, b: u8) -> u8 {
        let k = self.step();
        self.last_cipher = b ^ k;
        self.last_plain = b;
        self.last_cipher
    }

    pub fn decrypt_in_place(&mut self, buf: &mut [u8]) {
        for x in buf {
            *x = self.decrypt(*x);
        }
    }

    pub fn encrypt_in_place(&mut self, buf: &mut [u8]) {
        for x in buf {
            *x = self.encrypt(*x);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::books::VerseRef;
    use crate::model::TranslationMeta;
    use std::io::Write;

    /// Builds a zText (or RawText) module in memory from `(ref, osis fragment)` pairs.
    pub fn build_module(
        verses: &[(VerseRef, &str)],
        raw: bool,
        key: Option<&str>,
        conf_extra: &str,
    ) -> MemFiles {
        let mut files = HashMap::new();
        let dir = "modules/texts/ztext/test";
        for (t, books) in [("ot", 0..39u8), ("nt", 39..66u8)] {
            // Compute the index size for this testament.
            let mut entries: Vec<Vec<u8>> = vec![Vec::new(), Vec::new()];
            for book in books.map(BookId) {
                entries.push(Vec::new());
                for ch in 1..=book.chapters() {
                    entries.push(Vec::new());
                    let c = ChapterIdx::new(book, ch).unwrap();
                    for v in 1..=c.kjv_verses() {
                        let text = verses
                            .iter()
                            .find(|(r, _)| *r == VerseRef::new(book, ch, v))
                            .map(|(_, s)| s.as_bytes().to_vec())
                            .unwrap_or_default();
                        entries.push(text);
                    }
                }
            }
            if raw {
                let mut data = Vec::new();
                let mut index = Vec::new();
                for e in &entries {
                    let mut e = e.clone();
                    if let Some(k) = key {
                        Sapphire::new(k.as_bytes()).encrypt_in_place(&mut e);
                    }
                    index.extend_from_slice(&(data.len() as u32).to_le_bytes());
                    index.extend_from_slice(&(e.len() as u16).to_le_bytes());
                    data.extend_from_slice(&e);
                }
                files.insert(format!("{dir}/{t}.vss"), index);
                files.insert(format!("{dir}/{t}"), data);
            } else {
                // One block per 50 entries.
                let (mut bzs, mut bzv, mut bzz) = (Vec::new(), Vec::new(), Vec::new());
                for (bi, chunk) in entries.chunks(50).enumerate() {
                    let mut plain = Vec::new();
                    for e in chunk {
                        bzv.extend_from_slice(&(bi as u32).to_le_bytes());
                        bzv.extend_from_slice(&(plain.len() as u32).to_le_bytes());
                        bzv.extend_from_slice(&(e.len() as u16).to_le_bytes());
                        plain.extend_from_slice(e);
                    }
                    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                    enc.write_all(&plain).unwrap();
                    let mut comp = enc.finish().unwrap();
                    if let Some(k) = key {
                        Sapphire::new(k.as_bytes()).encrypt_in_place(&mut comp);
                    }
                    bzs.extend_from_slice(&(bzz.len() as u32).to_le_bytes());
                    bzs.extend_from_slice(&(comp.len() as u32).to_le_bytes());
                    bzs.extend_from_slice(&(plain.len() as u32).to_le_bytes());
                    bzz.extend_from_slice(&comp);
                }
                files.insert(format!("{dir}/{t}.bzs"), bzs);
                files.insert(format!("{dir}/{t}.bzv"), bzv);
                files.insert(format!("{dir}/{t}.bzz"), bzz);
            }
        }
        let conf = format!(
            "[Test]\nDataPath=./{dir}/\nModDrv={}\nSourceType=OSIS\nEncoding=UTF-8\nCompressType=ZIP\nBlockType=BOOK\nDescription=Test Module\nVersification=KJV\n{conf_extra}",
            if raw { "RawText" } else { "zText" }
        );
        files.insert("mods.d/test.conf".into(), conf.into_bytes());
        MemFiles(files)
    }

    fn fixture() -> Vec<(VerseRef, &'static str)> {
        vec![
            (VerseRef::new(BookId::GENESIS, 1, 1), "In the beginning God created the heaven and the earth."),
            (
                VerseRef::new(BookId::PSALMS, 23, 1),
                "<title type=\"psalm\" canonical=\"true\">A Psalm of David.</title><l level=\"1\">The <divineName>Lord</divineName> is my shepherd; I shall not want.</l>",
            ),
            (
                VerseRef::new(BookId(42), 3, 16),
                "For God so loved the world, <note>n</note><q who=\"Jesus\" marker=\"\">that he gave</q>",
            ),
            (
                VerseRef::new(BookId::REVELATION, 22, 21),
                "The grace of our Lord Jesus Christ be with you all. Amen.",
            ),
        ]
    }

    fn import(files: &MemFiles, key: Option<&str>) -> Result<crate::model::Translation> {
        let conf = SwordConf::parse(std::str::from_utf8(&files.read("mods.d/test.conf")?)?)?;
        let mut b = TranslationBuilder::new(TranslationMeta::default());
        read_module(&mut b, &conf, files, key)?;
        Ok(b.finish())
    }

    #[test]
    fn reads_ztext_and_rawtext() {
        for raw in [false, true] {
            let files = build_module(&fixture(), raw, None, "");
            let t = import(&files, None).unwrap();
            assert_eq!(t.verse_count(), 4);
            assert_eq!(t.find(VerseRef::new(BookId::GENESIS, 1, 1)).unwrap().text, fixture()[0].1);
            let ps = t.find(VerseRef::new(BookId::PSALMS, 23, 1)).unwrap();
            assert_eq!(ps.text, "The Lord is my shepherd; I shall not want.");
            assert_eq!(ps.marks[0].kind, MarkKind::PsalmTitle);
            let jn = t.find(VerseRef::new(BookId(42), 3, 16)).unwrap();
            assert_eq!(jn.text, "For God so loved the world, that he gave");
            assert_eq!(jn.spans[0].style, Style::WordsOfJesus);
            assert!(t.find(VerseRef::new(BookId::REVELATION, 22, 21)).is_some());
        }
    }

    #[test]
    fn locked_modules_need_the_right_key() {
        let files = build_module(&fixture(), false, Some("s3cret-Key"), "CipherKey=\n");
        assert!(import(&files, None).unwrap_err().to_string().contains("locked"));
        assert!(import(&files, Some("wrong")).is_err());
        let t = import(&files, Some("s3cret-Key")).unwrap();
        assert_eq!(t.verse_count(), 4);
        let files = build_module(&fixture(), true, Some("k"), "CipherKey=k\n");
        assert_eq!(import(&files, None).unwrap().verse_count(), 4);
    }

    #[test]
    fn sapphire_matches_sword_reference_vector() {
        // Produced by SWORD's own src/modules/common/sapphire.cpp (encrypt, key "s3cret-Key").
        let expected = "c89e99c63528af3a109d5e0038245651262c36b68c73016046b3a9b2d9957119034efb5579c5bf9421534acb6d4214503b5113588c89";
        let mut buf = b"In the beginning God created the heaven and the earth.".to_vec();
        Sapphire::new(b"s3cret-Key").encrypt_in_place(&mut buf);
        let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, expected);
    }

    #[test]
    fn sapphire_round_trip() {
        let msg = b"The quick brown fox jumps over the lazy dog".to_vec();
        let mut enc = msg.clone();
        Sapphire::new(b"key").encrypt_in_place(&mut enc);
        assert_ne!(enc, msg);
        let mut dec = enc.clone();
        Sapphire::new(b"key").decrypt_in_place(&mut dec);
        assert_eq!(dec, msg);
    }

    #[test]
    fn nrsv_versification() {
        let total: u32 = ChapterIdx::all().map(|c| versification("NRSV").unwrap()(c) as u32).sum();
        assert_eq!(total, 31_104);
        assert_eq!(versification("nrsva").unwrap()(ChapterIdx::new(BookId(63), 1).unwrap()), 15);
        assert_eq!(versification("NRSV").unwrap()(ChapterIdx::new(BookId::REVELATION, 12).unwrap()), 18);
        assert!(versification("Vulg").is_none());
    }

    #[test]
    fn conf_parsing() {
        let c = SwordConf::parse("[KJV]\r\nDataPath=./modules/texts/ztext/kjv/\nAbout=Line one\\\nline two\nCipherKey=\nDescription=King James\n").unwrap();
        assert_eq!(c.module, "KJV");
        assert_eq!(c.get("datapath"), Some("./modules/texts/ztext/kjv/"));
        assert_eq!(c.get("About"), Some("Line one\nline two"));
        assert!(c.is_locked());
        assert_eq!(c.description(), "King James");
    }

    fn zlib(plain: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(plain).unwrap();
        enc.finish().unwrap()
    }

    fn conf(extra: &str) -> SwordConf {
        SwordConf::parse(&format!(
            "[Test]\nDataPath=./modules/test/\nSourceType=OSIS\nEncoding=UTF-8\n{extra}"
        ))
        .unwrap()
    }

    #[test]
    fn commentary_entries_follow_the_linked_slots() {
        // A zCom module with only a New Testament: slot 0 the module's heading, 1 the testament's,
        // 2 Matthew's, 3 Matthew 1's, then one slot per verse. One block holds every text.
        let texts: [&[u8]; 3] =
            [b"<title>Matthew 1</title><p>On the chapter.</p>", b"On verses 1-2.", b"On verse 4."];
        let mut plain = Vec::new();
        let mut at = Vec::new();
        for t in texts {
            at.push((plain.len() as u32, t.len() as u16));
            plain.extend_from_slice(t);
        }
        let comp = zlib(&plain);
        let mut bzs = Vec::new();
        bzs.extend_from_slice(&0u32.to_le_bytes());
        bzs.extend_from_slice(&(comp.len() as u32).to_le_bytes());
        bzs.extend_from_slice(&(plain.len() as u32).to_le_bytes());
        let none = (0u32, 0u16);
        // Slots 4 and 5 (Matt 1:1, 1:2) share a record; slot 6 (1:3) has none; slot 7 is 1:4.
        let mut bzv = Vec::new();
        for rec in [none, none, none, at[0], at[1], at[1], none, at[2]] {
            bzv.extend_from_slice(&0u32.to_le_bytes());
            bzv.extend_from_slice(&rec.0.to_le_bytes());
            bzv.extend_from_slice(&rec.1.to_le_bytes());
        }
        let files = MemFiles(HashMap::from([
            ("modules/test/nt.bzs".to_string(), bzs),
            ("modules/test/nt.bzv".to_string(), bzv),
            ("modules/test/nt.bzz".to_string(), comp),
        ]));
        let matt = BookId(39);
        let entries = read_commentary(&conf("ModDrv=zCom\n"), &files).unwrap();
        assert_eq!(
            entries,
            vec![
                CommentaryEntry {
                    from: VerseRef::new(matt, 1, 1),
                    to: VerseRef::new(matt, 1, 25),
                    heading: Some("Matthew 1".into()),
                    text: "On the chapter.".into(),
                },
                CommentaryEntry {
                    from: VerseRef::new(matt, 1, 1),
                    to: VerseRef::new(matt, 1, 2),
                    heading: None,
                    text: "On verses 1-2.".into(),
                },
                CommentaryEntry {
                    from: VerseRef::new(matt, 1, 4),
                    to: VerseRef::new(matt, 1, 4),
                    heading: None,
                    text: "On verse 4.".into(),
                },
            ]
        );
    }

    #[test]
    fn lexicon_entries_keep_their_names_and_citations() {
        let entries: [(&str, &[u8]); 2] = [
            (
                "AARON",
                b"<entryFree n=\"Aaron\"><def>Eldest son of Amram <ref osisRef=\"Exod.6.20\">Ex. 6:20</ref>, see <ref osisRef=\"Exod.6.20\">6:20</ref> and <ref osisRef=\"Num.20.22-Num.20.29\">Num. 20:22-29</ref>.<note>A note<ref osisRef=\"Gen.1.1\">x</ref></note></def></entryFree>",
            ),
            ("ZUZIM", b"A people east of the Jordan."),
        ];
        let mut block = Vec::new();
        block.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        let mut data = Vec::new();
        let header = 4 + entries.len() * 8;
        for (_, text) in entries {
            block.extend_from_slice(&((header + data.len()) as u32).to_le_bytes());
            block.extend_from_slice(&((text.len() + 1) as u32).to_le_bytes());
            data.extend_from_slice(text);
            data.push(0);
        }
        block.extend_from_slice(&data);
        let comp = zlib(&block);
        let mut zdx = Vec::new();
        zdx.extend_from_slice(&0u32.to_le_bytes());
        zdx.extend_from_slice(&(comp.len() as u32).to_le_bytes());
        let (mut idx, mut dat) = (Vec::new(), Vec::new());
        for (n, (key, _)) in entries.iter().enumerate() {
            let mut rec = format!("{key}\n").into_bytes();
            rec.extend_from_slice(&0u32.to_le_bytes());
            rec.extend_from_slice(&(n as u32).to_le_bytes());
            idx.extend_from_slice(&(dat.len() as u32).to_le_bytes());
            idx.extend_from_slice(&(rec.len() as u32).to_le_bytes());
            dat.extend_from_slice(&rec);
        }
        let files = MemFiles(HashMap::from([
            ("modules/test.idx".to_string(), idx),
            ("modules/test.dat".to_string(), dat),
            ("modules/test.zdx".to_string(), zdx),
            ("modules/test.zdt".to_string(), comp),
        ]));
        let c =
            SwordConf::parse("[Test]\nDataPath=./modules/test\nModDrv=zLD\nSourceType=TEI\nEncoding=UTF-8\n")
                .unwrap();
        let read = read_lexicon(&c, &files).unwrap();
        let (exod, num) = (BookId(1), BookId(3));
        assert_eq!(
            read,
            vec![
                LexiconEntry {
                    key: "AARON".into(),
                    name: "Aaron".into(),
                    text: "Eldest son of Amram Ex. 6:20, see 6:20 and Num. 20:22-29.".into(),
                    cites: vec![
                        (VerseRef::new(exod, 6, 20), VerseRef::new(exod, 6, 20)),
                        (VerseRef::new(num, 20, 22), VerseRef::new(num, 20, 29)),
                        (VerseRef::new(BookId::GENESIS, 1, 1), VerseRef::new(BookId::GENESIS, 1, 1)),
                    ],
                },
                LexiconEntry {
                    key: "ZUZIM".into(),
                    name: "ZUZIM".into(),
                    text: "A people east of the Jordan.".into(),
                    cites: vec![],
                },
            ]
        );
    }

    #[test]
    fn genbook_nodes_come_depth_first() {
        // root -> "Chapter 1" (with child "Section 1") -> "Chapter 2"; the root has no text.
        const NONE: u32 = u32::MAX;
        let bdt = b"first\0second\0third\0".to_vec();
        let (mut idx, mut dat) = (Vec::new(), Vec::new());
        // (parent, next, child, name, text (offset, size)), each pushed in `.idx` order.
        let nodes = [
            (NONE, NONE, 4u32, "", None::<(u32, u32)>),
            (0, 8, 12, "Chapter 1", Some((0, 6))),
            (0, NONE, NONE, "Chapter 2", Some((6, 7))),
            (4, NONE, NONE, "Section 1", Some((13, 6))),
        ];
        let mut offsets = Vec::new();
        for (parent, next, child, name, text) in nodes {
            offsets.push(dat.len() as u32);
            for n in [parent, next, child] {
                dat.extend_from_slice(&n.to_le_bytes());
            }
            dat.extend_from_slice(name.as_bytes());
            dat.push(0);
            match text {
                Some((off, size)) => {
                    dat.extend_from_slice(&8u16.to_le_bytes());
                    dat.extend_from_slice(&off.to_le_bytes());
                    dat.extend_from_slice(&size.to_le_bytes());
                }
                None => dat.extend_from_slice(&0u16.to_le_bytes()),
            }
        }
        // A link is a byte offset into `.idx`: the root's record at 0, then 4, 8 and 12.
        for off in offsets {
            idx.extend_from_slice(&off.to_le_bytes());
        }
        let files = MemFiles(HashMap::from([
            ("modules/test.idx".to_string(), idx),
            ("modules/test.dat".to_string(), dat),
            ("modules/test.bdt".to_string(), bdt),
        ]));
        let c =
            SwordConf::parse("[Test]\nDataPath=./modules/test\nModDrv=RawGenBook\nEncoding=UTF-8\n").unwrap();
        let path = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            read_genbook(&c, &files).unwrap(),
            vec![
                GenBookNode { path: path(&["Chapter 1"]), raw: "first".into() },
                GenBookNode { path: path(&["Chapter 1", "Section 1"]), raw: "third".into() },
                GenBookNode { path: path(&["Chapter 2"]), raw: "second".into() },
            ]
        );
    }

    #[test]
    fn osis_ranges_land_on_the_kjv_grid() {
        let r = |b: BookId, c1: u8, v1: u8, c2: u8, v2: u8| {
            Some((VerseRef::new(b, c1, v1), VerseRef::new(b, c2, v2)))
        };
        let (gn, exod, matt, john) = (BookId::GENESIS, BookId(1), BookId(39), BookId(42));
        assert_eq!(osis_range("Bible:Exod.6.20"), r(exod, 6, 20, 6, 20));
        assert_eq!(osis_range("Exod.6.16-Exod.6.20"), r(exod, 6, 16, 6, 20));
        assert_eq!(osis_range("Gen.1.1-5"), r(gn, 1, 1, 1, 5));
        assert_eq!(osis_range("Gen.1.1-2.3"), r(gn, 1, 1, 2, 3));
        assert_eq!(osis_range("Matt.22"), r(matt, 22, 1, 22, 46));
        assert_eq!(osis_range("Gen"), r(gn, 1, 1, 50, 26));
        assert_eq!(osis_range("Ps.3.0"), r(BookId::PSALMS, 3, 1, 3, 1));
        assert_eq!(osis_range("John.3.99"), r(john, 3, 36, 3, 36));
        assert_eq!(osis_range("Tob.1.1"), None);
    }

    #[test]
    fn gbf_and_thml() {
        let mut b = TranslationBuilder::new(TranslationMeta::default());
        b.begin_book(BookId(42));
        b.begin_chapter(3);
        b.begin_verse(16, 16);
        gbf(&mut b, "For God<WG2316> so loved<RF>note<Rf> the world, <FR>that he gave<Fr>");
        b.begin_verse(17, 17);
        thml(&mut b, "For God <scripRef>x</scripRef>sent <font color=\"red\">not</font> his<br/>Son");
        let t = b.finish();
        let v = t.find(VerseRef::new(BookId(42), 3, 16)).unwrap();
        assert_eq!(v.text, "For God so loved the world, that he gave");
        assert_eq!(v.spans[0].style, Style::WordsOfJesus);
        let v = t.find(VerseRef::new(BookId(42), 3, 17)).unwrap();
        assert_eq!(v.plain_text(), "For God sent not his Son");
        assert_eq!(v.spans[0].style, Style::WordsOfJesus);
    }
}
