//! Cross-references: for each verse, the other passages that speak to it, ranked by how many
//! readers judged the link helpful. Ask's `cross_references` tool reads them, so the assistant can
//! let Scripture interpret Scripture from a vetted list instead of from memory.
//!
//! Built by `cargo xtask build-crossrefs` from OpenBible.info's cross-references (CC BY), which
//! start from the Treasury of Scripture Knowledge and are ranked by readers' votes. Verses are on
//! the KJV grid; provenance and licence are in `LICENSES/README.md`.
//!
//! The file is zstd-compressed UTF-8 text: a [`MAGIC`] line, then for each verse with
//! cross-references a `=book.chapter.verse` line (book index from 0) followed by one line per
//! link, most votes first: `book.chapter.verse \t votes`, or `book.chapter.verse-book.chapter.verse
//! \t votes` for a passage.

use anyhow::{Context as _, Result, bail};

use crate::books::{BookId, ChapterIdx, VerseRef};

pub const MAGIC: &str = "linger-crossrefs 1";

/// One cross-reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub from: VerseRef,
    /// The passage it points to, `to..=to_end` (the same verse for a single verse).
    pub to: VerseRef,
    pub to_end: VerseRef,
    /// Readers' net votes for the link.
    pub votes: i32,
}

/// Every cross-reference, decoded.
pub struct CrossRefs {
    /// Sorted by source verse; each verse's links most votes first.
    links: Vec<Link>,
}

impl CrossRefs {
    pub fn decode(compressed: &[u8]) -> Result<Self> {
        let raw = crate::format::decompress_all(compressed).context("decompressing the cross-references")?;
        Self::parse(std::str::from_utf8(&raw).context("the cross-references aren't UTF-8")?)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        if lines.next() != Some(MAGIC) {
            bail!("not a cross-reference file");
        }
        let mut links = Vec::new();
        let mut from: Option<VerseRef> = None;
        for line in lines {
            if let Some(v) = line.strip_prefix('=') {
                let v = parse_verse(v).with_context(|| format!("bad verse line {line:?}"))?;
                if from.is_some_and(|f| f >= v) {
                    bail!("verses out of order at {line:?}");
                }
                from = Some(v);
                continue;
            }
            let from = from.context("a link before the first verse")?;
            let (passage, votes) = line.split_once('\t').with_context(|| format!("bad link {line:?}"))?;
            let (a, b) = passage.split_once('-').unwrap_or((passage, passage));
            let bad = || format!("bad link {line:?}");
            links.push(Link {
                from,
                to: parse_verse(a).with_context(bad)?,
                to_end: parse_verse(b).with_context(bad)?,
                votes: votes.parse().with_context(bad)?,
            });
        }
        Ok(Self { links })
    }

    /// The bundled cross-references ([`bundled::CROSSREFS`](crate::bundled::CROSSREFS)), decoded
    /// the first time they're asked for. An error (the web hasn't fetched them yet) isn't kept.
    pub fn bundled() -> Result<&'static CrossRefs> {
        use std::sync::OnceLock;
        static DECODED: OnceLock<CrossRefs> = OnceLock::new();
        if let Some(c) = DECODED.get() {
            return Ok(c);
        }
        let decoded = CrossRefs::decode(&crate::bundled::bytes(crate::bundled::CROSSREFS)?)?;
        Ok(DECODED.get_or_init(|| decoded))
    }

    pub fn len(&self) -> usize {
        self.links.len()
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// The links from the verses `from..=to`, in verse order and most votes first within a verse.
    pub fn from_range(&self, from: VerseRef, to: VerseRef) -> &[Link] {
        let a = self.links.partition_point(|l| l.from < from);
        let b = self.links.partition_point(|l| l.from <= to);
        &self.links[a..b.max(a)]
    }

    /// The text form [`CrossRefs::parse`] reads, for links sorted as [`CrossRefs`] keeps them.
    pub fn to_text(links: &[Link]) -> String {
        let mut out = String::from(MAGIC);
        out.push('\n');
        let mut from = None;
        for l in links {
            if from != Some(l.from) {
                out.push_str(&format!("={}\n", verse_text(l.from)));
                from = Some(l.from);
            }
            out.push_str(&verse_text(l.to));
            if l.to_end != l.to {
                out.push_str(&format!("-{}", verse_text(l.to_end)));
            }
            out.push_str(&format!("\t{}\n", l.votes));
        }
        out
    }
}

fn verse_text(v: VerseRef) -> String {
    format!("{}.{}.{}", v.book.index(), v.chapter, v.verse)
}

/// `book.chapter.verse`, which must be on the KJV grid.
fn parse_verse(s: &str) -> Option<VerseRef> {
    let mut n = s.split('.').map(|x| x.parse::<u8>().ok());
    let (Some(Some(b)), Some(Some(c)), Some(Some(v)), None) = (n.next(), n.next(), n.next(), n.next()) else {
        return None;
    };
    let book = BookId::new(b as usize)?;
    let chapter = ChapterIdx::new(book, c)?;
    (v >= 1 && v <= chapter.kjv_verses()).then(|| VerseRef::new(book, c, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(book: u8, chapter: u8, verse: u8) -> VerseRef {
        VerseRef::new(BookId(book), chapter, verse)
    }

    #[test]
    fn round_trips_and_finds_a_passage_s_links() {
        let links = [
            Link { from: v(0, 1, 1), to: v(65, 14, 7), to_end: v(65, 14, 7), votes: 82 },
            Link { from: v(0, 1, 1), to: v(18, 134, 2), to_end: v(18, 134, 3), votes: 60 },
            Link { from: v(0, 1, 2), to: v(42, 1, 1), to_end: v(42, 1, 3), votes: 5 },
            Link { from: v(42, 3, 16), to: v(44, 5, 8), to_end: v(44, 5, 8), votes: -1 },
        ];
        let text = CrossRefs::to_text(&links);
        assert!(text.contains("=0.1.1\n65.14.7\t82\n18.134.2-18.134.3\t60\n"), "{text}");
        let x = CrossRefs::parse(&text).unwrap();
        assert_eq!(x.len(), 4);
        assert_eq!(x.from_range(v(0, 1, 1), v(0, 1, 1)), &links[..2]);
        assert_eq!(x.from_range(v(0, 1, 1), v(0, 1, 31)), &links[..3]);
        assert_eq!(x.from_range(v(42, 3, 16), v(42, 3, 16))[0].votes, -1);
        assert!(x.from_range(v(1, 1, 1), v(1, 40, 38)).is_empty());
    }

    #[test]
    fn rejects_verses_off_the_kjv_grid_and_out_of_order() {
        assert!(
            CrossRefs::parse("linger-crossrefs 1\n=63.1.15\n0.1.1\t3\n").is_err(),
            "3 John has 14 verses"
        );
        assert!(CrossRefs::parse("linger-crossrefs 1\n=0.1.2\n0.1.1\t3\n=0.1.1\n").is_err());
        assert!(CrossRefs::parse("something else\n").is_err());
    }
}
