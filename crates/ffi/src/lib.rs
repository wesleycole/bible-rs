//! Bindings from `linger-core` to Swift (and later Kotlin), generated with UniFFI.
//!
//! The mobile apps see a small, value-typed surface: a [`BibleLibrary`] that lists translations,
//! returns a chapter as plain verses and searches, plus free functions for the book grid and
//! reference parsing. Core types stay in Rust; everything crossing the boundary is a record or an
//! enum that UniFFI copies.
//!
//! Books are 0-based indices into the canonical 66-book grid: Genesis is 0, John 42, Revelation 65.
//! Chapter and verse numbers are 1-based, as printed.
//!
//! Decoding a translation (~20 ms) and building its search corpus (~35 ms, on the first search)
//! happen on first use, so call the library off the main thread.

use std::path::Path;
use std::sync::Arc;

use linger_core::books::{BookId, ChapterIdx};
use linger_core::library::{self, Library};
use linger_core::model::{MarkKind, Origin, VerseView};
use linger_core::{reference, search};

uniffi::setup_scaffolding!();

mod account;
mod study;

pub use account::{Account, AccountError, AccountInfo};
pub use study::{
    ChatMessage, ChatSummary, ConsentText, HighlightColor, LibrarySync, Note, Study, StudyObserver,
    SyncObserver, SyncState, TokenProvider, VerseMarks, VerseNote, VerseRef, highlight_colors,
    library_consent_text,
};

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum BibleError {
    #[error("translation {id:?} is not installed")]
    UnknownTranslation { id: String },
    #[error("book {book}, chapter {chapter} is not in the canon")]
    InvalidChapter { book: u8, chapter: u8 },
    #[error("{message}")]
    Failed { message: String },
}

/// Told when something the library returned may have changed, e.g. an online chapter that was
/// `Pending` has arrived. Called from a background thread.
#[uniffi::export(with_foreign)]
pub trait LibraryObserver: Send + Sync {
    fn library_changed(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Testament {
    Old,
    New,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Book {
    /// 0-based position in the canon.
    pub index: u8,
    /// USFM code: `GEN`, `1CO`, …
    pub code: String,
    pub name: String,
    pub abbrev: String,
    pub chapters: u8,
    pub testament: Testament,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct Reference {
    pub book: u8,
    pub chapter: u8,
    pub verse: Option<u8>,
    pub verse_end: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TranslationKind {
    Bundled,
    Imported,
    Online,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TranslationInfo {
    pub id: String,
    pub abbrev: String,
    pub name: String,
    /// BCP-47 language tag.
    pub language: String,
    pub license: String,
    pub copyright: String,
    pub kind: TranslationKind,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Verse {
    pub number: u8,
    /// Equal to `number` unless the translation merges verses ("16–17").
    pub number_end: u8,
    /// The verse as plain text: poetry line breaks become spaces, styles are dropped.
    pub text: String,
    /// Section headings and Psalm superscriptions that come before this verse.
    pub headings: Vec<String>,
    /// The verse begins a new paragraph.
    pub starts_paragraph: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ChapterContent {
    /// `notice` is the publisher's notice for online translations, which must be shown with the
    /// text.
    Ready {
        verses: Vec<Verse>,
        notice: Option<String>,
    },
    /// An online chapter is being fetched; the observer is told when it arrives.
    Pending,
    Failed {
        message: String,
    },
    /// An online translation that needs an API key.
    NoKey,
    /// The translation doesn't contain this chapter (e.g. a New Testament only import).
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SearchScope {
    All,
    OldTestament,
    NewTestament,
    Book { book: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SearchHit {
    pub reference: Reference,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SearchResults {
    /// The first `limit` matches, in canonical order.
    pub hits: Vec<SearchHit>,
    /// Every match, including those past `limit`.
    pub total: u32,
    pub elapsed_ms: f64,
}

/// The installed translations: bundled, imported into `data_dir`, and online ones configured
/// there.
#[derive(uniffi::Object)]
pub struct BibleLibrary {
    inner: Library,
}

#[uniffi::export]
impl BibleLibrary {
    /// Opens the library stored in `data_dir` (on iOS, a folder in Application Support).
    #[uniffi::constructor]
    pub fn new(data_dir: String, observer: Option<Arc<dyn LibraryObserver>>) -> Self {
        let notify: Arc<dyn Fn() + Send + Sync> = match observer {
            Some(o) => Arc::new(move || o.library_changed()),
            None => Arc::new(|| {}),
        };
        Self { inner: Library::open(Path::new(&data_dir), notify) }
    }

    pub fn translations(&self) -> Vec<TranslationInfo> {
        self.inner
            .entries()
            .iter()
            .map(|e| TranslationInfo {
                id: e.meta.id.clone(),
                abbrev: e.meta.abbrev.clone(),
                name: e.meta.name.clone(),
                language: e.meta.language.clone(),
                license: e.meta.license.clone(),
                copyright: e.copyright(),
                kind: match e.meta.origin {
                    Origin::Bundled => TranslationKind::Bundled,
                    Origin::Imported { .. } => TranslationKind::Imported,
                    Origin::Online { .. } => TranslationKind::Online,
                },
            })
            .collect()
    }

    pub fn chapter(&self, translation: String, book: u8, chapter: u8) -> Result<ChapterContent, BibleError> {
        let entry = self.entry(&translation)?;
        let c = chapter_idx(book, chapter)?;
        Ok(match self.inner.chapter(&translation, c) {
            library::Chapter::Ready(t) => ChapterContent::Ready {
                verses: t.verses_in(c).map(|v| verse(&t, &v)).collect(),
                notice: (!entry.is_local()).then(|| entry.copyright()),
            },
            library::Chapter::Pending => ChapterContent::Pending,
            library::Chapter::Failed(message) => ChapterContent::Failed { message },
            library::Chapter::NoKey => ChapterContent::NoKey,
            library::Chapter::Missing => ChapterContent::Missing,
        })
    }

    /// Full-text search of a local translation. Words match as prefixes unless `whole_words`;
    /// `"quoted phrases"` and `-excluded` words work as in the desktop app.
    pub fn search(
        &self,
        translation: String,
        query: String,
        scope: SearchScope,
        whole_words: bool,
        limit: u32,
    ) -> Result<SearchResults, BibleError> {
        let entry = self.entry(&translation)?;
        if !entry.is_local() {
            return Err(BibleError::Failed { message: "online translations can't be searched".into() });
        }
        let t = entry.load().map_err(|e| BibleError::Failed { message: format!("{e:#}") })?;
        let scope = match scope {
            SearchScope::All => search::Scope::All,
            SearchScope::OldTestament => search::Scope::OldTestament,
            SearchScope::NewTestament => search::Scope::NewTestament,
            SearchScope::Book { book } => search::Scope::Book(book_id(book)?),
        };
        let outcome = search::run(&t, &query, scope, whole_words);
        let hits = outcome
            .verses
            .iter()
            .take(limit as usize)
            .filter_map(|&i| t.verse_view(i))
            .map(|v| SearchHit {
                reference: Reference {
                    book: v.chapter.book().0,
                    chapter: v.chapter.chapter(),
                    verse: Some(v.number),
                    verse_end: (v.number_end > v.number).then_some(v.number_end),
                },
                text: v.plain_text(),
            })
            .collect();
        Ok(SearchResults {
            hits,
            total: outcome.verses.len() as u32,
            elapsed_ms: outcome.elapsed.as_secs_f64() * 1000.0,
        })
    }
}

impl BibleLibrary {
    fn entry(&self, id: &str) -> Result<&Arc<library::Entry>, BibleError> {
        self.inner.get(id).ok_or_else(|| BibleError::UnknownTranslation { id: id.into() })
    }
}

/// The translation a new reader starts with.
#[uniffi::export]
pub fn default_translation_id() -> String {
    linger_core::bundled::DEFAULT_ID.into()
}

/// The 66 books of the canon, Genesis first.
#[uniffi::export]
pub fn books() -> Vec<Book> {
    BookId::all()
        .map(|b| Book {
            index: b.0,
            code: b.info().code.into(),
            name: b.name().into(),
            abbrev: b.info().abbrev.into(),
            chapters: b.chapters(),
            testament: match b.testament() {
                linger_core::Testament::Old => Testament::Old,
                linger_core::Testament::New => Testament::New,
            },
        })
        .collect()
}

/// Parses a typed reference: "jn 3:16", "Rom 8", "1co13:4-7", "revalation 21".
#[uniffi::export]
pub fn parse_reference(input: String) -> Option<Reference> {
    reference::parse(&input).map(Reference::from)
}

/// Ranked readings of a partly typed reference, for suggestions as the user types.
#[uniffi::export]
pub fn suggest_references(input: String, limit: u32) -> Vec<Reference> {
    reference::suggest(&input, limit as usize).into_iter().map(Reference::from).collect()
}

/// "John 3:16–18", "Psalm 23".
#[uniffi::export]
pub fn format_reference(reference: Reference) -> Result<String, BibleError> {
    let r = reference::Reference {
        book: book_id(reference.book)?,
        chapter: reference.chapter,
        verse: reference.verse,
        verse_end: reference.verse_end,
    };
    chapter_idx(reference.book, reference.chapter)?;
    Ok(r.display())
}

/// A piece of text with its references found: what a note or a typed passage shows as links.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum TextSegment {
    Text { text: String },
    Reference { text: String, reference: Reference },
}

/// Splits text at the references written in it ("see Rom 8:28 and jn 3").
#[uniffi::export]
pub fn reference_segments(text: String) -> Vec<TextSegment> {
    let mut out = Vec::new();
    let mut at = 0;
    for (range, r) in reference::find_all(&text) {
        if range.start > at {
            out.push(TextSegment::Text { text: text[at..range.start].into() });
        }
        out.push(TextSegment::Reference { text: text[range.clone()].into(), reference: r.into() });
        at = range.end;
    }
    if at < text.len() {
        out.push(TextSegment::Text { text: text[at..].into() });
    }
    out
}

/// A piece of an answer from Ask.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CitationSegment {
    Text {
        text: String,
    },
    /// A `{{John 3:16}}` citation.
    Verse {
        reference: Reference,
    },
    /// A moment in a recorded sermon: its id (or the id's first characters) and where.
    Sermon {
        id: String,
        at_ms: u64,
    },
}

/// Splits an answer at its `{{…}}` citations.
#[uniffi::export]
pub fn citation_segments(text: String) -> Vec<CitationSegment> {
    linger_core::chat::segments(&text)
        .into_iter()
        .map(|s| match s {
            linger_core::chat::Segment::Text(text) => CitationSegment::Text { text },
            linger_core::chat::Segment::Verse(r) => CitationSegment::Verse { reference: r.into() },
            linger_core::chat::Segment::Sermon { id, at_ms } => CitationSegment::Sermon { id, at_ms },
        })
        .collect()
}

impl From<reference::Reference> for Reference {
    fn from(r: reference::Reference) -> Self {
        Self { book: r.book.0, chapter: r.chapter, verse: r.verse, verse_end: r.verse_end }
    }
}

fn book_id(book: u8) -> Result<BookId, BibleError> {
    BookId::new(book as usize).ok_or(BibleError::InvalidChapter { book, chapter: 0 })
}

fn chapter_idx(book: u8, chapter: u8) -> Result<ChapterIdx, BibleError> {
    ChapterIdx::new(book_id(book)?, chapter).ok_or(BibleError::InvalidChapter { book, chapter })
}

fn verse(t: &linger_core::Translation, v: &VerseView<'_>) -> Verse {
    Verse {
        number: v.number,
        number_end: v.number_end,
        text: v.plain_text(),
        headings: v
            .marks
            .iter()
            .filter(|m| m.offset == 0 && matches!(m.kind, MarkKind::Heading | MarkKind::PsalmTitle))
            .map(|m| t.mark_text(m).to_string())
            .collect(),
        starts_paragraph: v
            .marks
            .iter()
            .any(|m| m.offset == 0 && matches!(m.kind, MarkKind::Paragraph | MarkKind::ParagraphFlush)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> BibleLibrary {
        let dir = std::env::temp_dir().join(format!("linger-ffi-{}", std::process::id()));
        BibleLibrary::new(dir.to_string_lossy().into_owned(), None)
    }

    #[test]
    fn reads_a_chapter_with_its_headings() {
        let lib = library();
        // Psalm 23 in the BSB: 6 verses, a superscription before verse 1.
        let ChapterContent::Ready { verses, notice } = lib.chapter("bsb".into(), 18, 23).unwrap() else {
            panic!("Psalm 23 should be ready");
        };
        assert_eq!(verses.len(), 6);
        assert_eq!(verses.iter().map(|v| v.number).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6]);
        assert!(verses[0].headings.iter().any(|h| h.contains("David")), "{:?}", verses[0].headings);
        assert!(verses[0].text.contains("shepherd"));
        assert_eq!(notice, None);
    }

    #[test]
    fn rejects_chapters_outside_the_canon() {
        let lib = library();
        // Jude has one chapter; book 66 doesn't exist.
        assert!(matches!(
            lib.chapter("kjv".into(), 64, 2),
            Err(BibleError::InvalidChapter { book: 64, chapter: 2 })
        ));
        assert!(matches!(lib.chapter("kjv".into(), 66, 1), Err(BibleError::InvalidChapter { .. })));
        assert!(matches!(lib.chapter("nope".into(), 0, 1), Err(BibleError::UnknownTranslation { .. })));
    }

    #[test]
    fn search_limits_hits_but_counts_all() {
        let lib = library();
        let r = lib.search("kjv".into(), "\"jesus wept\"".into(), SearchScope::All, false, 10).unwrap();
        assert_eq!(r.total, 1);
        assert_eq!(
            r.hits[0].reference,
            Reference { book: 42, chapter: 11, verse: Some(35), verse_end: None }
        );

        let r = lib.search("kjv".into(), "faith".into(), SearchScope::Book { book: 57 }, true, 3).unwrap();
        assert_eq!(r.hits.len(), 3);
        assert!(r.total > 3, "Hebrews mentions faith more than three times");
        assert!(r.hits.iter().all(|h| h.reference.book == 57));
    }

    #[test]
    fn parses_and_formats_references() {
        let r = parse_reference("1co13:4-7".into()).unwrap();
        assert_eq!(r, Reference { book: 45, chapter: 13, verse: Some(4), verse_end: Some(7) });
        assert_eq!(format_reference(r).unwrap(), "1 Corinthians 13:4–7");
        assert!(format_reference(Reference { book: 42, chapter: 22, verse: None, verse_end: None }).is_err());
        assert_eq!(books().len(), 66);
        assert_eq!(books()[39].testament, Testament::New);
    }

    #[test]
    fn splits_text_at_references_and_citations() {
        let rom = Reference { book: 44, chapter: 8, verse: Some(28), verse_end: None };
        let jn = Reference { book: 42, chapter: 3, verse: None, verse_end: None };
        assert_eq!(
            reference_segments("See Rom 8:28 and John 3.".into()),
            [
                TextSegment::Text { text: "See ".into() },
                TextSegment::Reference { text: "Rom 8:28".into(), reference: rom },
                TextSegment::Text { text: " and ".into() },
                TextSegment::Reference { text: "John 3".into(), reference: jn },
                TextSegment::Text { text: ".".into() },
            ]
        );
        assert_eq!(
            citation_segments("As {{Romans 8:28}} says, {{sermon 1a2b3c4d 12:30}}{{not one}}".into()),
            [
                CitationSegment::Text { text: "As ".into() },
                CitationSegment::Verse { reference: rom },
                CitationSegment::Text { text: " says, ".into() },
                CitationSegment::Sermon { id: "1a2b3c4d".into(), at_ms: 750_000 },
                CitationSegment::Text { text: "{{not one}}".into() },
            ]
        );
    }
}
