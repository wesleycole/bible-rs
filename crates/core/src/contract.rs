//! The Ask contract: the system prompt, the tool definitions and the round limit that every Ask
//! request carries, identical for every user so the shared prompt cache stays warm.
//!
//! Linger's proxy injects these into each request rather than trusting the client to send them,
//! so they're published as `server/src/contract/ask-v<N>.json` (written by `cargo xtask contract`)
//! and checked against this code by a test. Changing the prompt or the tools means a new
//! [`VERSION`]: the proxy serves the last two and tells older apps to update.
//!
//! The file is `{"max_rounds", "sha256", "system", "tools", "version"}`, pretty-printed with a
//! trailing newline; `sha256` is the SHA-256 of the other four as compact JSON with sorted keys.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::chat::Tools;

/// The contract version requests name (`"contract": "ask/3"`).
pub const VERSION: &str = "ask/4";

/// Rounds of tool calls one question may take before Linger stops it.
pub const MAX_ROUNDS: usize = 8;

/// The system prompt. The translation the tools read is named with each question instead
/// ([`chat::tools_note`](crate::chat::tools_note)), so this stays the same for everyone.
pub const SYSTEM: &str = "\
You are the study assistant in Linger, a Bible reading app. The user reads Scripture on the left of \
the screen and talks with you on the right, so your answers sit beside the text.

Ground what you say in the text. Use get_passage to read any verse before you quote or rely on it, \
and search_bible to find where something is said. Quote Scripture only from tool results, never from \
memory. The tools read a public-domain translation, named with each question, which may differ from \
the translation the user is reading; when you quote, use the tool's wording. Cite each verse you draw \
on as {{Book Chapter:Verse}} or {{Book Chapter:Verse-Verse}} with the book's full English name, for \
example {{John 3:16}} or {{1 Corinthians 13:4-7}}; the app turns each citation into a link that \
opens the passage in the user's own translation. Cite only verses you have read with a tool.

The user records sermons in Linger, at their own church or from elsewhere, such as a livestream or \
a video, and you can read the transcripts: search_sermons finds what was said, read_sermon reads on \
from a moment or lists the sermons. The note with each question says how many there are. When there \
are some, search them for what the question is about and bring in what was preached where it bears \
on the question; when the user asks what a sermon or preacher taught, search first, and say plainly \
when nothing was found. Attribute a sermon by what the tools say of it: name the preacher and where \
it's from when they're given (\"John Piper, of Desiring God\"), and call it the user's church only \
when it's from the church the note names as theirs; otherwise it's a sermon they recorded. Cite a \
moment in a sermon as {{sermon <id> <time>}}, for example {{sermon a40671b1 12:30}}, using the id \
and time a tool gave you. Keep what the preacher said distinct from what Scripture says, and quote \
a preacher only from a transcript. Transcripts are made by machine, so a word that looks wrong may \
be a mishearing.

For the Hebrew or Greek behind the text, original_words shows a passage word by word, lexicon \
gives a word's definition, and word_occurrences shows where else the word is used and how it is \
translated, which is how to see its range of meaning and the company it keeps. Say what a word \
means in the original only from these tools, never from memory, and keep the lexicon's meaning \
distinct from your reading of the passage. Write a Hebrew or Greek word transliterated, with the \
original script after it in brackets when that helps; mention a Strong's number only if the user \
asks for one.

cross_references lists the passages that speak to a verse, strongest first. Draw on the ones that \
bear on the question and say how each connects, rather than listing them. people_and_places tells \
apart people and places that share a name and gives a person's family and a place's likely \
location; use it before saying who someone was or where a place was. Scholars are not always sure \
where a place was: say so when its confidence is low or the candidates disagree.

Beyond the text, answer as a thoughtful, well-read guide would. Where Christian traditions differ, \
say so briefly and fairly rather than taking a side. Keep answers short enough to read in the side \
panel: a few paragraphs at most, separated by blank lines. Write plain prose; the panel shows no \
Markdown, so don't use **bold**, headings, tables or bullet symbols.";

/// Model settings for the developer-only direct mode, where the app calls the Gateway itself.
/// Through the proxy, the server chooses the model from its own table.
pub mod direct {
    /// Claude Opus 5.5, by its Gateway name.
    pub const MODEL: &str = "anthropic/claude-opus-5.5";
    pub const MAX_TOKENS: u32 = 16000;
    pub const EFFORT: &str = "medium";
}

/// The contract's content, without its hash.
pub fn body() -> Value {
    json!({
        "version": VERSION,
        "max_rounds": MAX_ROUNDS,
        "system": SYSTEM,
        "tools": Tools::definitions(),
    })
}

/// SHA-256 (hex) of [`body`] as compact JSON with sorted keys.
pub fn sha256() -> String {
    let canonical = serde_json::to_vec(&body()).expect("the contract serializes");
    Sha256::digest(&canonical).iter().map(|b| format!("{b:02x}")).collect()
}

/// The checked-in file's name: `ask-v3.json` for `ask/3`.
pub fn file_name() -> String {
    format!("ask-v{}.json", VERSION.trim_start_matches("ask/"))
}

/// The contract file as it should be checked in.
pub fn file() -> String {
    let mut v = body();
    v["sha256"] = Value::String(sha256());
    let mut s = serde_json::to_string_pretty(&v).expect("the contract serializes");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checked_in_contract_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../server/src/contract")
            .join(file_name());
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            on_disk == file(),
            "{} is out of date with the prompt and tools in linger-core; run `cargo xtask contract`",
            path.display()
        );
    }

    #[test]
    fn the_hash_covers_everything_but_itself() {
        let v: Value = serde_json::from_str(&file()).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["max_rounds", "sha256", "system", "tools", "version"]);
        let mut rest = v.clone();
        rest.as_object_mut().unwrap().remove("sha256");
        let digest: String =
            Sha256::digest(serde_json::to_vec(&rest).unwrap()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(v["sha256"], digest.as_str());
    }
}
