//! `linger-core` in the browser (docs/plans/m8-web.md, "Spike first"). The page fetches the data
//! files and hands them in with [`add_data`]; everything else is core, called through
//! wasm-bindgen with JSON in and out. There is no logic here.
//!
//! Build: `cargo build -p linger-web --release --target wasm32-unknown-unknown`, then
//! `wasm-bindgen --target web --out-dir crates/web/www/pkg target/wasm32-unknown-unknown/release/linger_web.wasm`.

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};
use linger_core::bundled;
use linger_core::chat::{self, Tools};
use linger_core::model::Translation;
use linger_core::reference;
use linger_core::search::{self, Scope};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

/// The files the page fetched, by their path under `data/`, kept for the page's lifetime.
static FILES: Mutex<Vec<(String, &'static [u8])>> = Mutex::new(Vec::new());
static TRANSLATIONS: Mutex<Vec<Arc<Translation>>> = Mutex::new(Vec::new());

struct Fetched;

impl bundled::Source for Fetched {
    fn bytes(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        FILES.lock().ok()?.iter().find(|(p, _)| p == path).map(|(_, b)| Cow::Borrowed(*b))
    }
}

fn js<T>(r: Result<T>) -> Result<T, JsError> {
    r.map_err(|e| JsError::new(&format!("{e:#}")))
}

/// Hands core one fetched file: `path` is relative to `data/` (`translations/bsb.bbl`,
/// `crossrefs/crossrefs.zst`).
#[wasm_bindgen]
pub fn add_data(path: &str, bytes: Vec<u8>) -> Result<(), JsError> {
    let mut files = FILES.lock().unwrap();
    if files.is_empty() {
        js(bundled::install(Box::new(Fetched)))?;
    }
    files.push((path.to_string(), Box::leak(bytes.into_boxed_slice())));
    Ok(())
}

/// Decodes a translation added with [`add_data`]; returns its metadata as JSON.
#[wasm_bindgen]
pub fn load_translation(id: &str) -> Result<String, JsError> {
    let t = js(bundled::load(id))?;
    let meta = serde_json::to_string(&t.meta).unwrap_or_default();
    TRANSLATIONS.lock().unwrap().push(Arc::new(t));
    Ok(meta)
}

fn translation(id: &str) -> Result<Arc<Translation>> {
    TRANSLATIONS
        .lock()
        .unwrap()
        .iter()
        .find(|t| t.id() == id)
        .cloned()
        .with_context(|| format!("{id} isn't loaded"))
}

/// A chapter as the reader draws it: `[{number, number_end, text, marks: [{offset, kind, level,
/// text}], spans: [{start, end, style}]}]`, with `kind` and `style` as `model::MarkKind` and
/// `model::Style` discriminants.
#[wasm_bindgen]
pub fn chapter(id: &str, reference: &str) -> Result<String, JsError> {
    let t = js(translation(id))?;
    let r = js(reference::parse(reference).with_context(|| format!("not a reference: {reference:?}")))?;
    let verses: Vec<Value> = t
        .verses_in(r.chapter_idx())
        .map(|v| {
            let marks: Vec<Value> = v
                .marks
                .iter()
                .map(|m| json!({ "offset": m.offset, "kind": m.kind as u8, "level": m.level, "text": t.mark_text(m) }))
                .collect();
            let spans: Vec<Value> =
                v.spans.iter().map(|s| json!({ "start": s.start, "end": s.end, "style": s.style as u8 })).collect();
            json!({ "number": v.number, "number_end": v.number_end, "text": v.text, "marks": marks, "spans": spans })
        })
        .collect();
    Ok(Value::Array(verses).to_string())
}

/// Full-text search over the whole translation: `{total, verses: [{reference, text}],
/// elapsed_ms}`. The first search also builds the index.
#[wasm_bindgen]
pub fn search(id: &str, query: &str, limit: usize) -> Result<String, JsError> {
    let t = js(translation(id))?;
    let outcome = search::run(&t, query, Scope::All, false);
    let verses: Vec<Value> = outcome
        .verses
        .iter()
        .take(limit)
        .filter_map(|&i| t.verse_view(i))
        .map(|v| json!({ "reference": v.reference().to_string(), "text": v.plain_text() }))
        .collect();
    let elapsed_ms = outcome.elapsed.as_secs_f64() * 1000.0;
    Ok(json!({ "total": outcome.verses.len(), "verses": verses, "elapsed_ms": elapsed_ms }).to_string())
}

/// What the user typed, as a reference (`"jn3:16"` → `{display: "John 3:16", book, chapter,
/// verse, verse_end}`), or null.
#[wasm_bindgen]
pub fn parse_reference(input: &str) -> Option<String> {
    let r = reference::parse(input)?;
    let v = json!({ "display": r.display(), "book": r.book.0, "chapter": r.chapter, "verse": r.verse, "verse_end": r.verse_end });
    Some(v.to_string())
}

/// An answer's text split at its `{{John 3:16}}` citations: `[{text} | {reference} | {item,
/// at_ms?, page?} | {sermon, at_ms}]`.
#[wasm_bindgen]
pub fn segments(text: &str) -> String {
    let segments: Vec<Value> = chat::segments(text)
        .into_iter()
        .map(|s| match s {
            chat::Segment::Text(t) => json!({ "text": t }),
            chat::Segment::Verse(r) => json!({ "reference": r.display() }),
            chat::Segment::Item { uid, at_ms, page } => json!({ "item": uid, "at_ms": at_ms, "page": page }),
            chat::Segment::Sermon { id, at_ms } => json!({ "sermon": id, "at_ms": at_ms }),
        })
        .collect();
    Value::Array(segments).to_string()
}

/// Runs one of Ask's device-side tools (`cross_references`, `get_passage`, `search_bible`…)
/// against a loaded translation, with the input the model would send; returns the tool's result.
#[wasm_bindgen]
pub fn run_tool(id: &str, name: &str, input: &str) -> Result<String, JsError> {
    let tools = Tools { translation: js(translation(id))?, library: None, church: None };
    let input: Value = js(serde_json::from_str(input).context("the tool input isn't JSON"))?;
    Ok(js(tools.run(name, &input))?.to_string())
}
