//! The people, places and other proper names of the Bible, each individual told apart: the thirty
//! Zechariahs are 29 entries. Each has the forms of its name in English and in Hebrew or Greek
//! (with the Strong's number [`crate::original`] tags it with), every verse it is named in, its
//! family, and for a place where modern scholarship locates it and how confidently. Ask's
//! `people_and_places` tool reads it.
//!
//! Built by `cargo xtask build-names` from STEPBible's TIPNR (people, places and their names and
//! references) and OpenBible.info's Bible Geocoding Data (where each place was), both CC BY 4.0;
//! provenance and licences are in `LICENSES/README.md`. Verses are on the KJV grid.
//!
//! The file is zstd-compressed UTF-8: a [`MAGIC`] line, then one [`Entity`] per line as JSON.

use std::collections::HashMap;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::books::VerseRef;
use crate::original;

pub const MAGIC: &str = "linger-names 1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Person,
    Place,
    /// Deities, months, titles, musical terms and other capitalized words.
    Other,
}

/// One individual person, place or other named thing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    /// Its most common English name and where it is first named: "Aaron@Exo.4.14" (STEPBible's
    /// book abbreviations).
    pub id: String,
    pub category: Category,
    /// "Male", "Female", "Group", "Place", "Supernatural", "Time", …
    pub kind: String,
    /// "High Priest living at the time of Egypt and Wilderness".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// A person's tribe or nation; a place's region.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tribe: String,
    /// Relations are ids, with " (ancestor)", " (founder)" or " (uncertain)" where the link is
    /// not simply to a parent or is a judgment call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub father: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mother: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub siblings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partners: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offspring: Vec<String>,
    /// For a place: who founded or rebuilt it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub founders: Vec<String>,
    /// For a place: the people and peoples who lived there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inhabitants: Vec<String>,
    pub names: Vec<Form>,
    /// For a place: where it may have been, most likely first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<Location>,
}

/// One form of a name: a Hebrew or Greek word and how translations render it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form {
    /// English renderings, the ESV's first: ["Elijah", "Elias"].
    pub english: Vec<String>,
    pub original: String,
    /// The disambiguated Strong's number, as [`crate::original`] tags the word.
    pub strong: String,
    /// The verses it is in, as [`VerseRef::key`]s in canonical order.
    pub verses: Vec<u32>,
}

/// A modern location identified with an ancient place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    /// "Qalunya", "near Jericho", "another name for Jerusalem".
    pub identification: String,
    /// "settlement", "river", "mountain", "region", …
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// How confident current scholarship is in the identification, 0–100 (OpenBible.info's
    /// time-weighted score).
    pub confidence: u8,
    /// Latitude and longitude, when the data has a point for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat_lon: Option<(f64, f64)>,
}

impl Entity {
    /// Every verse the entity is named in, in canonical order.
    pub fn verses(&self) -> Vec<VerseRef> {
        let mut keys: Vec<u32> = self.names.iter().flat_map(|f| f.verses.iter().copied()).collect();
        keys.sort_unstable();
        keys.dedup();
        keys.into_iter().filter_map(VerseRef::from_key).collect()
    }

    /// The English name the entity is known by: "Aaron" for "Aaron@Exo.4.14".
    pub fn name(&self) -> &str {
        self.id.split('@').next().unwrap_or(&self.id)
    }
}

/// Every name, decoded and indexed.
pub struct Names {
    entities: Vec<Entity>,
    by_id: HashMap<String, usize>,
    by_name: HashMap<String, Vec<usize>>,
    by_strong: HashMap<String, Vec<usize>>,
}

impl Names {
    pub fn decode(compressed: &[u8]) -> Result<Self> {
        let raw = zstd::decode_all(compressed).context("decompressing the names")?;
        Self::parse(std::str::from_utf8(&raw).context("the names aren't UTF-8")?)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        if lines.next() != Some(MAGIC) {
            bail!("not a names file");
        }
        let entities = lines
            .enumerate()
            .map(|(i, l)| serde_json::from_str(l).with_context(|| format!("entity {}", i + 1)))
            .collect::<Result<Vec<Entity>>>()?;
        Ok(Self::new(entities))
    }

    pub fn new(entities: Vec<Entity>) -> Self {
        let mut by_id = HashMap::new();
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_strong: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, e) in entities.iter().enumerate() {
            by_id.insert(id_key(&e.id), i);
            let mut keys: Vec<String> = std::iter::once(e.name())
                .chain(e.names.iter().flat_map(|f| f.english.iter().map(String::as_str)))
                .map(name_key)
                .filter(|k| !k.is_empty())
                .collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                by_name.entry(k).or_default().push(i);
            }
            for f in &e.names {
                let list = by_strong.entry(f.strong.clone()).or_default();
                if list.last() != Some(&i) {
                    list.push(i);
                }
            }
        }
        Self { entities, by_id, by_name, by_strong }
    }

    /// The bundled names, decoded the first time they're asked for.
    #[cfg(feature = "bundled")]
    pub fn bundled() -> Result<&'static Names> {
        use std::sync::OnceLock;
        static BYTES: &[u8] =
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/names/names.zst"));
        static DECODED: OnceLock<std::result::Result<Names, String>> = OnceLock::new();
        DECODED
            .get_or_init(|| Names::decode(BYTES).map_err(|e| format!("{e:#}")))
            .as_ref()
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn all(&self) -> &[Entity] {
        &self.entities
    }

    pub fn get(&self, id: &str) -> Option<&Entity> {
        self.by_id.get(&id_key(id)).map(|&i| &self.entities[i])
    }

    /// The entities a query names, most often named first: an id ("Aaron@Exo.4.14"), a Strong's
    /// number ("H0452G"; "H0452" finds every sense), or an English name in any translation's
    /// spelling, ignoring case, spaces and hyphens ("Beth-lehem" is "Bethlehem").
    pub fn lookup(&self, query: &str) -> Vec<&Entity> {
        let mut found: Vec<usize> = if query.contains('@') {
            self.by_id.get(&id_key(query)).copied().into_iter().collect()
        } else if let Some(s) = original::normalize_strong(query) {
            match self.by_strong.get(&s) {
                Some(list) => list.clone(),
                None => {
                    let mut all: Vec<usize> = self
                        .by_strong
                        .iter()
                        .filter(|(k, _)| original::base(k) == s)
                        .flat_map(|(_, v)| v.iter().copied())
                        .collect();
                    all.sort_unstable();
                    all.dedup();
                    all
                }
            }
        } else {
            self.by_name.get(&name_key(query)).cloned().unwrap_or_default()
        };
        found.sort_by_cached_key(|&i| (std::cmp::Reverse(self.entities[i].verses().len()), i));
        found.into_iter().map(|i| &self.entities[i]).collect()
    }

    /// The entities named in the verses `from..=to`, in the order they are first named there.
    pub fn named_in(&self, from: VerseRef, to: VerseRef) -> Vec<&Entity> {
        let (a, b) = (from.key(), to.key());
        let mut found: Vec<(u32, usize)> = self
            .entities
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let first =
                    e.names.iter().flat_map(|f| f.verses.iter()).filter(|k| (a..=b).contains(*k)).min()?;
                Some((*first, i))
            })
            .collect();
        found.sort_unstable();
        found.into_iter().map(|(_, i)| &self.entities[i]).collect()
    }
}

/// An id without what follows its first reference: "Aaron@Exo.4.14-Heb (ancestor)" →
/// "aaron@exo.4.14".
fn id_key(id: &str) -> String {
    let id = id.trim();
    let (name, rest) = id.split_once('@').unwrap_or((id, ""));
    let reference = rest.split(['-', ' ', '(']).next().unwrap_or_default();
    format!("{}@{}", name_key(name), reference.to_lowercase())
}

/// A name as typed or translated, compared without case, spaces, hyphens or apostrophes.
fn name_key(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::books::BookId;

    fn form(english: &[&str], strong: &str, verses: &[VerseRef]) -> Form {
        Form {
            english: english.iter().map(|s| s.to_string()).collect(),
            original: String::new(),
            strong: strong.into(),
            verses: verses.iter().map(|v| v.key()).collect(),
        }
    }

    fn entity(id: &str, category: Category, names: Vec<Form>) -> Entity {
        Entity {
            id: id.into(),
            category,
            kind: "Male".into(),
            description: String::new(),
            tribe: String::new(),
            father: None,
            mother: None,
            siblings: vec![],
            partners: vec![],
            offspring: vec![],
            founders: vec![],
            inhabitants: vec![],
            names,
            locations: vec![],
        }
    }

    fn sample() -> Names {
        let kings = |c, v| VerseRef::new(BookId(10), c, v);
        let ezra = VerseRef::new(BookId(14), 10, 21);
        let matthew = VerseRef::new(BookId(39), 17, 3);
        let bethlehem = VerseRef::new(BookId(7), 1, 1);
        let text = [
            MAGIC.to_string(),
            serde_json::to_string(&entity(
                "Elijah@Ezr.10.21",
                Category::Person,
                vec![form(&["Elijah"], "H0452I", &[ezra])],
            ))
            .unwrap(),
            serde_json::to_string(&entity(
                "Elijah@1Ki.17.1-Jas",
                Category::Person,
                vec![
                    form(&["Elijah"], "H0452G", &[kings(17, 1), kings(18, 1)]),
                    form(&["Elias", "Elijah"], "G2243", &[matthew]),
                ],
            ))
            .unwrap(),
            serde_json::to_string(&entity(
                "Bethlehem@Gen.35.16-Jhn",
                Category::Place,
                vec![form(&["Bethlehem", "Beth-lehem"], "H1035G", &[bethlehem])],
            ))
            .unwrap(),
        ]
        .join("\n");
        Names::parse(&text).unwrap()
    }

    #[test]
    fn finds_individuals_by_name_number_and_id() {
        let n = sample();
        let elijahs: Vec<&str> = n.lookup("elijah").iter().map(|e| e.id.as_str()).collect();
        assert_eq!(elijahs, ["Elijah@1Ki.17.1-Jas", "Elijah@Ezr.10.21"], "most often named first");
        assert_eq!(n.lookup("Elias")[0].id, "Elijah@1Ki.17.1-Jas", "the KJV's spelling");
        assert_eq!(n.lookup("beth lehem")[0].category, Category::Place);
        assert_eq!(n.lookup("h452i")[0].id, "Elijah@Ezr.10.21");
        assert_eq!(n.lookup("H0452").len(), 2, "a number without its letter finds every individual");
        assert_eq!(n.lookup("Elijah@1Ki.17.1")[0].id, "Elijah@1Ki.17.1-Jas", "an id without its range");
        assert_eq!(n.lookup("Elijah@1Ki.17.1-Jas (ancestor)").len(), 1, "a relation as given");
        assert!(n.lookup("Elisha").is_empty());
        assert!(n.lookup("Elijah@Gen.1.1").is_empty());
    }

    #[test]
    fn finds_who_is_named_in_a_passage() {
        let n = sample();
        let v = |b, c, x| VerseRef::new(BookId(b), c, x);
        let named = n.named_in(v(10, 17, 1), v(10, 18, 46));
        assert_eq!(named.len(), 1);
        assert_eq!(named[0].verses().len(), 3);
        assert!(n.named_in(v(10, 17, 2), v(10, 17, 24)).is_empty());
    }
}
