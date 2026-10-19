//! `cargo xtask build-crossrefs`: OpenBible.info's cross-references for Ask's `cross_references`
//! tool (see `linger_core::crossrefs` for the file format).
//!
//! OpenBible.info (CC BY) regenerates the file every day as votes come in, so the pin below
//! changes when it is refreshed: `XTASK_ACCEPT_UPDATED_SOURCES=1` takes the current file and
//! prints its hash. Links with no net votes in their favour are left out, as are verses off the
//! KJV grid (a range that runs past a KJV chapter's end stops at it).

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use linger_core::books::{BookId, ChapterIdx, VerseRef};
use linger_core::crossrefs::{CrossRefs, Link};

const URL: &str = "https://a.openbible.info/data/cross-references.zip";
/// The file as downloaded on 2026-10-19.
const SHA256: &str = "5f61f4525156b7c7b8f7ae07e3a4225038cf4b60def39247df8b92916e14c56f";

pub fn build(root: &Path) -> Result<()> {
    let zip = crate::fetch_pinned(root, "openbible-cross-references.zip", URL, SHA256)?;
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip)?)?;
    let mut text = String::new();
    archive.by_name("cross_references.txt")?.read_to_string(&mut text)?;

    let mut lines = text.lines();
    let header = lines.next().context("empty file")?;
    if !header.starts_with("From Verse\tTo Verse\tVotes") {
        bail!("unexpected header {header:?}");
    }
    let (mut unvoted, mut off_grid, mut clamped) = (0, 0, 0);
    let mut links = Vec::new();
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        let [from, to, votes] = f[..] else { bail!("bad line {line:?}") };
        let votes: i32 = votes.trim().parse().with_context(|| format!("bad votes in {line:?}"))?;
        if votes <= 0 {
            unvoted += 1;
            continue;
        }
        let (a, b) = to.split_once('-').unwrap_or((to, to));
        let (Some(from), Some(to)) = (verse(from)?, verse(a)?) else {
            off_grid += 1;
            continue;
        };
        let to_end = match verse(b)? {
            Some(end) => end,
            // Past the end of a KJV chapter: the chapter's last verse.
            None => {
                clamped += 1;
                let (book, chapter, _) = osis(b)?;
                let c = ChapterIdx::new(book, chapter).with_context(|| format!("no chapter in {line:?}"))?;
                VerseRef::new(book, chapter, c.kjv_verses())
            }
        };
        if to_end < to {
            bail!("backwards range in {line:?}");
        }
        links.push(Link { from, to, to_end, votes });
    }
    links.sort_by(|a, b| a.from.cmp(&b.from).then(b.votes.cmp(&a.votes)).then(a.to.cmp(&b.to)));
    links.dedup_by(|a, b| a.from == b.from && a.to == b.to && a.to_end == b.to_end);

    let out = CrossRefs::to_text(&links);
    let decoded = CrossRefs::parse(&out)?;
    let bytes = zstd::encode_all(out.as_bytes(), 19)?;
    let dir = root.join("data/crossrefs");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("crossrefs.zst"), &bytes)?;
    println!(
        "{} cross-references: {:.1} MB text → {:.2} MB; left out {unvoted} without net votes and {off_grid} off the \
         KJV grid; {clamped} ranges stop at a KJV chapter's end",
        decoded.len(),
        out.len() as f64 / 1e6,
        bytes.len() as f64 / 1e6
    );
    Ok(())
}

/// "Gen.1.1" → its book, chapter and verse numbers.
fn osis(r: &str) -> Result<(BookId, u8, u8)> {
    let mut p = r.trim().split('.');
    let (Some(b), Some(c), Some(v), None) = (p.next(), p.next(), p.next(), p.next()) else {
        bail!("bad reference {r:?}");
    };
    let book = BookId::from_osis(b).with_context(|| format!("unknown book in {r:?}"))?;
    let num = |s: &str| s.parse::<u8>().with_context(|| format!("bad number in {r:?}"));
    Ok((book, num(c)?, num(v)?))
}

/// The verse, or `None` if it isn't on the KJV grid (3 John 1:15, Revelation 12:18).
fn verse(r: &str) -> Result<Option<VerseRef>> {
    let (book, chapter, verse) = osis(r)?;
    let ok = ChapterIdx::new(book, chapter).is_some_and(|c| (1..=c.kjv_verses()).contains(&verse));
    Ok(ok.then(|| VerseRef::new(book, chapter, verse)))
}
