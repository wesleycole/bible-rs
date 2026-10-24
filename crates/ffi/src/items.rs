//! Library items from the cache (M3b's FFI 8): what Library, the margin popover and the verse
//! page show, as the desktop's Library page reads them. Filtering stays here, not in Swift.

use linger_core::records::{
    self, AnchorOrigin as CoreOrigin, ItemKind as CoreKind, ItemStatus as CoreStatus,
};
use linger_core::study::Body;

use crate::study::{Study, VerseRef, core_verse, failed};
use crate::{BibleError, chapter_idx};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ItemKind {
    Sermon,
    Handwritten,
    Slide,
    Page,
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AnchorOrigin {
    User,
    Typed,
    Model,
}

macro_rules! same_enum {
    ($ours:ident, $theirs:ty, $($v:ident),+) => {
        impl From<$theirs> for $ours {
            fn from(x: $theirs) -> Self {
                match x { $(<$theirs>::$v => Self::$v),+ }
            }
        }
        impl From<$ours> for $theirs {
            fn from(x: $ours) -> Self {
                match x { $($ours::$v => Self::$v),+ }
            }
        }
    };
}

same_enum!(ItemKind, CoreKind, Sermon, Handwritten, Slide, Page, Note);
same_enum!(ItemStatus, CoreStatus, Recording, Uploading, Waiting, Processing, Review, Ready, Error);
same_enum!(AnchorOrigin, CoreOrigin, User, Typed, Model);

/// A range of verses an item is anchored to.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ItemAnchor {
    pub from: VerseRef,
    pub to: VerseRef,
    /// "Romans 8:28–30".
    pub reference: String,
    pub origin: AnchorOrigin,
    /// Where in a sermon it is said, in milliseconds.
    pub at_ms: Option<u64>,
    /// The page of a photographed item.
    pub page: Option<u32>,
}

/// A library item as lists show it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LibraryItem {
    pub uid: String,
    pub kind: ItemKind,
    pub title: String,
    pub status: ItemStatus,
    pub excerpt: Option<String>,
    pub summary: Option<String>,
    pub outline: Option<String>,
    pub speaker: Option<String>,
    /// The church, ministry or channel it came from.
    pub from: Option<String>,
    /// When it was made or captured, in server milliseconds.
    pub captured_at: u64,
    /// A model wrote parts nobody has checked yet.
    pub unreviewed: bool,
    /// User anchors first.
    pub anchors: Vec<ItemAnchor>,
}

/// One turn of speech in a sermon's transcript.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TranscriptSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: Option<String>,
    pub text: String,
}

/// An item's text, as the cache holds it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ItemText {
    Plain { text: String },
    Transcript { segments: Vec<TranscriptSegment> },
}

/// An item anchored in a chapter: verses `first..=last` of it (an anchor that starts in the
/// chapter before shows from verse 1). An item anchored twice in a chapter has two.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ChapterItem {
    pub first: u8,
    pub last: u8,
    pub item: LibraryItem,
}

fn verse(id: records::VerseId) -> Option<VerseRef> {
    id.verse_ref().map(|v| VerseRef { book: v.book.0, chapter: v.chapter, verse: v.verse })
}

fn range(from: records::VerseId, to: records::VerseId) -> String {
    match (from.verse_ref(), to.verse_ref()) {
        (Some(a), Some(b)) if a == b => a.display(),
        (Some(a), Some(b)) if a.chapter_idx() == b.chapter_idx() => format!("{}–{}", a.display(), b.verse),
        (Some(a), Some(b)) => format!("{}–{}", a.display(), b.display()),
        _ => format!("{from}–{to}"),
    }
}

/// An item's title as lists show it: its own, an untitled note's first line, or its kind.
pub(crate) fn title_of(i: &records::Item) -> String {
    i.display_title().unwrap_or_else(|| {
        match i.kind {
            CoreKind::Sermon => "Sermon",
            CoreKind::Note => "Note",
            CoreKind::Handwritten | CoreKind::Page => "Photo",
            CoreKind::Slide => "Slide",
        }
        .into()
    })
}

impl From<records::Item> for LibraryItem {
    fn from(i: records::Item) -> Self {
        let title = title_of(&i);
        let mut anchors: Vec<ItemAnchor> = i
            .anchors
            .iter()
            .filter_map(|a| {
                Some(ItemAnchor {
                    from: verse(a.from)?,
                    to: verse(a.to)?,
                    reference: range(a.from, a.to),
                    origin: a.origin.into(),
                    at_ms: a.at_ms,
                    page: a.page,
                })
            })
            .collect();
        anchors.sort_by_key(|a| a.origin != AnchorOrigin::User);
        let details = i.details.as_ref();
        Self {
            uid: i.uid,
            kind: i.kind.into(),
            title,
            status: i.status.into(),
            excerpt: i.excerpt,
            summary: i.summary,
            outline: i.outline,
            speaker: details.and_then(|d| d.speaker.clone()),
            from: details.and_then(|d| d.from.clone()),
            captured_at: i.captured_at,
            unreviewed: i.unreviewed,
            anchors,
        }
    }
}

#[uniffi::export]
impl Study {
    /// The live items, newest first; only those of `kinds`, when given.
    pub fn library_items(&self, kinds: Option<Vec<ItemKind>>) -> Result<Vec<LibraryItem>, BibleError> {
        let items = self.lock().items().map_err(failed)?;
        Ok(items
            .into_iter()
            .filter(|i| kinds.as_ref().is_none_or(|k| k.contains(&i.kind.into())))
            .map(Into::into)
            .collect())
    }

    /// The item `uid` (or the one whose uid starts with it, as a citation gives it).
    pub fn library_item(&self, uid: String) -> Result<Option<LibraryItem>, BibleError> {
        Ok(self.lock().item(&uid).map_err(failed)?.map(Into::into))
    }

    /// An item's text: a note's own, or a sermon's or photo's once its file has been fetched.
    pub fn library_item_text(&self, uid: String) -> Result<Option<ItemText>, BibleError> {
        let s = self.lock();
        let Some(item) = s.item(&uid).map_err(failed)? else { return Ok(None) };
        Ok(s.body(&item).map_err(failed)?.map(|b| match b {
            Body::Plain(text) => ItemText::Plain { text },
            Body::Transcript(t) => ItemText::Transcript {
                segments: t
                    .segments
                    .into_iter()
                    .map(|s| TranscriptSegment {
                        start_ms: s.start_ms,
                        end_ms: s.end_ms,
                        speaker: s.speaker,
                        text: s.text,
                    })
                    .collect(),
            },
        }))
    }

    /// The items anchored in a chapter, by first verse: the margin glyphs and the popover.
    pub fn chapter_items(&self, book: u8, chapter: u8) -> Result<Vec<ChapterItem>, BibleError> {
        let c = chapter_idx(book, chapter)?;
        let s = self.lock();
        let mut out = Vec::new();
        for span in s.spans_in_chapter(c).map_err(failed)? {
            if let Some(item) = s.item(&span.uid).map_err(failed)? {
                out.push(ChapterItem { first: span.first, last: span.last, item: item.into() });
            }
        }
        Ok(out)
    }

    /// Saves a note in the library anchored to the ranges (each `from`…`to`, as selected), and to
    /// the references written in it; returns its uid. Synced like the rest.
    pub fn save_note_item(&self, text: String, ranges: Vec<ItemRange>) -> Result<String, BibleError> {
        let mut uid = String::new();
        self.write(|s| {
            let ranges = ranges
                .iter()
                .map(|r| Ok((core_verse(r.from)?, core_verse(r.to)?)))
                .collect::<Result<Vec<_>, BibleError>>()?;
            uid = s.save_note_item(&text, &ranges).map_err(failed)?;
            Ok(())
        })?;
        Ok(uid)
    }
}

/// A selected range, `from` ≤ `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct ItemRange {
    pub from: VerseRef,
    pub to: VerseRef,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_note_reads_back_through_the_chapter_and_the_list() {
        let dir = std::env::temp_dir().join(format!("linger-ffi-items-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let study = Study::new(dir.to_string_lossy().into_owned(), None).unwrap();
        let rom = |c, v| VerseRef { book: 44, chapter: c, verse: v };
        // Romans 8:28–30 as selected, and a reference typed in the text.
        let uid = study
            .save_note_item(
                "All things for good. See Genesis 50:20.".into(),
                vec![ItemRange { from: rom(8, 28), to: rom(8, 30) }],
            )
            .unwrap();

        let in_8: Vec<(u8, u8)> =
            study.chapter_items(44, 8).unwrap().iter().map(|c| (c.first, c.last)).collect();
        assert_eq!(in_8, [(28, 30)]);
        let in_gen_50 = study.chapter_items(0, 50).unwrap();
        assert_eq!(
            (in_gen_50[0].first, in_gen_50[0].last, in_gen_50[0].item.uid.as_str()),
            (20, 20, uid.as_str())
        );
        assert!(study.chapter_items(44, 9).unwrap().is_empty());

        let item = study.library_item(uid[..8].into()).unwrap().unwrap();
        assert_eq!(item.kind, ItemKind::Note);
        // Untitled, a note is listed by its first line.
        assert_eq!(item.title, "All things for good. See Genesis 50:20.");
        assert_eq!(item.anchors[0].reference, "Romans 8:28–30");
        assert_eq!(item.anchors[0].origin, AnchorOrigin::User);
        assert_eq!(
            (item.anchors[1].reference.as_str(), item.anchors[1].origin),
            ("Genesis 50:20", AnchorOrigin::Typed)
        );
        assert_eq!(
            study.library_item_text(uid.clone()).unwrap(),
            Some(ItemText::Plain { text: "All things for good. See Genesis 50:20.".into() })
        );
        assert_eq!(study.library_items(Some(vec![ItemKind::Sermon])).unwrap(), []);
        assert_eq!(study.library_items(None).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
