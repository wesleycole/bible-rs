//! Core of linger: the verse-indexed translation model, the `.bbl` binary format, importers,
//! full-text search, reference parsing and local study storage. No GUI code lives here.
//!
//! Modules marked `native` need the OS or a C library; the rest, with `recording` and
//! `transcribe` reduced to their types, also build for wasm32 (`--no-default-features`).

#[cfg(feature = "native")]
pub mod api;
pub mod appearance;
#[cfg(feature = "native")]
pub mod assistant;
#[cfg(feature = "native")]
pub mod auth;
pub mod books;
pub mod bundled;
pub mod chat;
pub mod contract;
pub mod copy;
pub mod crossrefs;
pub mod format;
#[cfg(feature = "native")]
pub mod import;
#[cfg(feature = "native")]
pub mod library;
pub mod model;
pub mod names;
#[cfg(feature = "native")]
pub mod online;
pub mod original;
pub mod plans;
pub mod privacy;
pub mod recording;
pub mod records;
pub mod reference;
pub mod search;
#[cfg(feature = "native")]
pub mod secrets;
#[cfg(feature = "native")]
pub mod study;
#[cfg(feature = "sync")]
pub mod sync;
pub mod time;
pub mod transcribe;
pub mod usfm;
pub mod xml;

pub use books::{BookId, ChapterIdx, Testament, VerseRef};
pub use model::{Translation, TranslationMeta};
