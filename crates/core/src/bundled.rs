//! Linger's own data: the public-domain translations (see `LICENSES/` for provenance) and the
//! Ask data (cross-references, names, the original-language text) under `data/`.
//!
//! Where the bytes come from is a [`Source`]: compiled into the binary with `include_bytes!`
//! (feature `bundled`, the desktop and the apps), or handed in after a fetch (the web, which
//! installs its source with [`install`] and adds each file as it arrives). Only the compressed
//! bytes are kept; a translation is decoded the first time it is used (≈10 ms), so startup only
//! pays for the one being read.

use std::borrow::Cow;
use std::sync::OnceLock;

use anyhow::Result;

use crate::format;
use crate::model::{Translation, TranslationMeta};

/// Where the files under `data/` come from.
pub trait Source: Send + Sync {
    /// The bytes of `path` (relative to `data/`: [`CROSSREFS`], [`translation_path`]), or `None`
    /// when this source doesn't have them (yet).
    fn bytes(&self, path: &str) -> Option<Cow<'static, [u8]>>;
}

pub const CROSSREFS: &str = "crossrefs/crossrefs.zst";
pub const NAMES: &str = "names/names.zst";
pub const ORIGINAL: &str = "original/original.zst";

/// `translations/<id>.bbl`.
pub fn translation_path(id: &str) -> String {
    format!("translations/{id}.bbl")
}

/// The translations, in the order they are offered to the reader.
pub const IDS: [&str; 7] = ["kjv", "web", "bsb", "asv", "ylt", "dby", "wbs"];

pub const DEFAULT_ID: &str = "kjv";

static SOURCE: OnceLock<Box<dyn Source>> = OnceLock::new();

/// Installs the process's data source, once; a second call is an error. Builds with the
/// `bundled` feature have the embedded source already and never need this.
pub fn install(source: Box<dyn Source>) -> Result<()> {
    SOURCE.set(source).map_err(|_| anyhow::anyhow!("the data source is already installed"))
}

fn source() -> Result<&'static dyn Source> {
    if let Some(s) = SOURCE.get() {
        return Ok(s.as_ref());
    }
    #[cfg(feature = "bundled")]
    return Ok(&Embedded);
    #[cfg(not(feature = "bundled"))]
    anyhow::bail!("this build has no data source installed")
}

/// The bytes of a file under `data/`.
pub fn bytes(path: &str) -> Result<Cow<'static, [u8]>> {
    source()?.bytes(path).ok_or_else(|| anyhow::anyhow!("no data for {path:?} (not loaded yet?)"))
}

#[cfg(feature = "bundled")]
macro_rules! bbl {
    ($id:literal) => {
        (
            $id,
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/translations/", $id, ".bbl"))
                as &[u8],
        )
    };
}

/// `(id, compressed bytes)`, in the order they are offered to the reader.
#[cfg(feature = "bundled")]
pub static BUNDLED: &[(&str, &[u8])] =
    &[bbl!("kjv"), bbl!("web"), bbl!("bsb"), bbl!("asv"), bbl!("ylt"), bbl!("dby"), bbl!("wbs")];

/// The files compiled into the binary.
#[cfg(feature = "bundled")]
struct Embedded;

#[cfg(feature = "bundled")]
impl Source for Embedded {
    fn bytes(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        let b: &'static [u8] = match path {
            CROSSREFS => {
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/", "crossrefs/crossrefs.zst"))
            }
            NAMES => include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/", "names/names.zst")),
            ORIGINAL => {
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/", "original/original.zst"))
            }
            _ => {
                let id = path.strip_prefix("translations/")?.strip_suffix(".bbl")?;
                BUNDLED.iter().find(|(i, _)| *i == id).map(|(_, b)| *b)?
            }
        };
        Some(Cow::Borrowed(b))
    }
}

/// The year of the edition a bundled text follows, shown beside its abbreviation.
pub fn year(id: &str) -> Option<u16> {
    Some(match id {
        "kjv" => 1769,
        "web" => 2020,
        "bsb" => 2023,
        "asv" => 1901,
        "ylt" => 1898,
        "dby" => 1890,
        "wbs" => 1833,
        _ => return None,
    })
}

/// A line about each bundled text for the translation library.
pub fn description(id: &str) -> Option<&'static str> {
    Some(match id {
        "kjv" => "Red letters · italics · small-caps LORD",
        "web" => "Red letters · modern English",
        "bsb" => "Section headings · modern English",
        "asv" => "Literal · 1901",
        "ylt" => "Very literal · 1898",
        "dby" => "Literal · 1890",
        "wbs" => "Revision of the KJV · 1833",
        _ => return None,
    })
}

/// The compressed `.bbl` of a bundled translation.
pub fn translation_bytes(id: &str) -> Result<Cow<'static, [u8]>> {
    if !IDS.contains(&id) {
        anyhow::bail!("no bundled translation {id:?}");
    }
    bytes(&translation_path(id))
}

pub fn load(id: &str) -> Result<Translation> {
    format::decode(&translation_bytes(id)?)
}

/// The metadata of every bundled translation the source has.
pub fn metas() -> Vec<TranslationMeta> {
    IDS.iter().filter_map(|id| format::read_meta(&translation_bytes(id).ok()?).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::books::{BookId, ChapterIdx, VerseRef};
    use crate::model::Origin;

    #[test]
    fn every_bundled_translation_is_complete_and_public_domain() {
        for id in IDS {
            let t = load(id).unwrap();
            assert_eq!(t.meta.id, id);
            assert_eq!(t.meta.origin, Origin::Bundled);
            assert!(t.meta.license.starts_with("Public Domain"), "{id}: {}", t.meta.license);
            assert!(t.verse_count() > 31_000, "{id} has {} verses", t.verse_count());
            for c in ChapterIdx::all() {
                assert!(t.has_chapter(c), "{id} is missing {}", c.display());
            }
            let first = t.find(VerseRef::new(BookId::GENESIS, 1, 1)).unwrap();
            assert!(first.text.contains("beginning"), "{id}: {}", first.text);
        }
    }

    #[test]
    fn red_letters_where_the_source_has_them() {
        let kjv = load("kjv").unwrap();
        assert!(kjv.meta.red_letters);
        let v = kjv.find(VerseRef::new(BookId(42), 3, 16)).unwrap();
        assert!(v.spans.iter().any(|s| s.style == crate::model::Style::WordsOfJesus));
        assert_eq!(
            v.text,
            "For God so loved the world, that he gave his only begotten Son, that whosoever believeth in him should not perish, but have everlasting life."
        );
    }
}
