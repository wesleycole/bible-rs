//! `cargo xtask build-names`: the Bible's people, places and other proper names for Ask's
//! `people_and_places` tool (see `linger_core::names` for the file format).
//!
//! Sources, pinned by commit and SHA-256 and downloaded into `.sources/`:
//!
//! - STEPBible TIPNR (Tyndale House, CC BY 4.0): every individual person and place, the forms of
//!   their names with Strong's numbers, every reference, and family relations.
//! - OpenBible.info Bible Geocoding Data (CC BY 4.0): the modern locations scholars identify
//!   with each place, how confidently, and their coordinates.
//!
//! Left out, and why (also in `LICENSES/README.md`): TIPNR's `@Brief`, `@Short` and `@Article`
//! descriptions, which STEPBible adapted from an AI model's output; references found only in the
//! Septuagint; and coordinates that OpenBible.info took from OpenStreetMap (ODbL, a share-alike
//! licence).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, bail};
use linger_core::books::{BookId, ChapterIdx, VerseRef};
use linger_core::names::{Category, Entity, Form, Location, MAGIC, Names};
use linger_core::original::{self, Original};
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

use crate::original::{BOOKS, File, fetch, step};

const TIPNR: File = step(
    "Proper Nouns/TIPNR - Translators Individualised Proper Names with all References - STEPBible.org CC BY.txt",
    "dc2fba41f24ed85ce0e29b426208e7ea06efc2b85ae8f2a14a06cda01648cd43",
);
const fn geo(path: &'static str, sha256: &'static str) -> File {
    File {
        repo: "openbibleinfo/Bible-Geocoding-Data",
        commit: "7eb18a5ee62f27b9b93bd6689ea272d76dd23b8f",
        path,
        sha256,
    }
}
const ANCIENT: File =
    geo("data/ancient.jsonl", "b8187aa4737e8517ccc090f765d2be11da4c548cd2a59d3cdcb62e952cb8c0f2");
const MODERN: File =
    geo("data/modern.jsonl", "da731f6e110bac4ea66a9f037a0a31cfb11c4f1efc1206aa9e109092b2c60087");

/// Identifications kept for a place, most likely first.
const LOCATIONS: usize = 4;

pub fn build(root: &Path) -> Result<()> {
    let read = |f: &File| -> Result<String> {
        let path = fetch(root, f)?;
        let s = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
        Ok(s.trim_start_matches('\u{feff}').replace('\r', ""))
    };
    let mut stats = Stats::default();
    let (mut entities, openbible_names) = tipnr(&read(&TIPNR)?, &mut stats)?;
    let places = geocoding(&read(&ANCIENT)?, &read(&MODERN)?)?;
    attach_locations(&mut entities, &openbible_names, places, &mut stats);

    // Every relation should name an entity.
    let ids: HashSet<String> = entities.iter().map(|e| key(&e.id)).collect();
    if ids.len() != entities.len() {
        bail!("{} ids are not unique", entities.len() - ids.len());
    }
    // Every relation should name an entity; the few that don't are dropped.
    let mut dangling = Vec::new();
    for e in &mut entities {
        let id = e.id.clone();
        let mut keep = |r: &String| {
            let ok = ids.contains(&key(r));
            if !ok {
                dangling.push(format!("{id} → {r}"));
            }
            ok
        };
        e.father = e.father.take().filter(&mut keep);
        e.mother = e.mother.take().filter(&mut keep);
        for list in [&mut e.siblings, &mut e.partners, &mut e.offspring, &mut e.founders, &mut e.inhabitants]
        {
            list.retain(&mut keep);
        }
    }

    let mut out = String::from(MAGIC);
    out.push('\n');
    for e in &entities {
        out.push_str(&serde_json::to_string(e)?);
        out.push('\n');
    }
    let out: String = out.nfc().collect();
    let decoded = Names::parse(&out)?;
    let bytes = zstd::encode_all(out.as_bytes(), 19)?;
    let dir = root.join("data/names");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("names.zst"), &bytes)?;

    let count = |c| decoded.all().iter().filter(|e| e.category == c).count();
    println!(
        "{} people, {} places, {} other names: {:.1} MB text → {:.2} MB",
        count(Category::Person),
        count(Category::Place),
        count(Category::Other),
        out.len() as f64 / 1e6,
        bytes.len() as f64 / 1e6
    );
    println!(
        "left out {} Septuagint-only references, {} entities named nowhere else; moved {} references past a KJV \
         chapter's end to its last verse; {} references unreadable: {:?}",
        stats.lxx,
        stats.empty.len(),
        stats.clamped,
        stats.unreadable.len(),
        stats.unreadable
    );
    println!(
        "places with a location: {} of {}; OpenBible.info places matched by the name TIPNR gives {}, by the \
         TIPNR id OpenBible.info gives {}; {} not used (another candidate for a place already matched, or not a \
         TIPNR place), such as {:?}",
        decoded.all().iter().filter(|e| !e.locations.is_empty()).count(),
        count(Category::Place),
        stats.matched_by_name,
        stats.matched_by_id,
        stats.unmatched.len(),
        &stats.unmatched[..stats.unmatched.len().min(12)]
    );
    println!("coordinates left out as OpenStreetMap-derived: {}", stats.osm);
    println!("relations naming no entity ({}): {:?}", dangling.len(), dangling);

    // The Strong's numbers should be the ones the Hebrew and Greek text is tagged with.
    let text = Original::decode(&std::fs::read(root.join("data/original/original.zst"))?)?;
    let mut untagged: Vec<&str> = decoded
        .all()
        .iter()
        .flat_map(|e| &e.names)
        .map(|f| f.strong.as_str())
        .filter(|s| text.count(s) == 0)
        .collect();
    untagged.sort();
    untagged.dedup();
    println!("name forms whose number tags no word of the text ({}): {:?}", untagged.len(), untagged);
    Ok(())
}

#[derive(Default)]
struct Stats {
    lxx: usize,
    clamped: usize,
    unreadable: Vec<String>,
    empty: Vec<String>,
    matched_by_name: usize,
    matched_by_id: usize,
    unmatched: Vec<String>,
    osm: usize,
}

/// TIPNR's records: a `$====… PERSON(s)` (or PLACE, PLACE+PERSON, OTHER) line, a header line of
/// tab-separated fields, then a `– Significance \t name \t dStrong«eStrong=original \t English \t
/// references` line for each form of the name, a `– Total` line and the `@…=` descriptions.
/// Returns each entity with the name OpenBible.info gives a place ("Bethlehem_1").
fn tipnr(text: &str, stats: &mut Stats) -> Result<(Vec<Entity>, Vec<String>)> {
    let mut out = Vec::new();
    let mut openbible = Vec::new();
    for record in text.split("\n$").skip(1) {
        let mut lines = record.lines();
        let category = match lines.next().unwrap_or_default().trim_matches(['=', ' ', '\t']) {
            "PERSON(s)" => Category::Person,
            "PLACE" | "PLACE+PERSON" => Category::Place,
            "OTHER" => Category::Other,
            _ => continue,
        };
        let header: Vec<&str> = lines.next().unwrap_or_default().split('\t').map(str::trim).collect();
        // The section's own description of its fields.
        if header[0].starts_with("UniqueName") || header[0].starts_with("UnifiedName") {
            continue;
        }
        let (id, _) = header[0].split_once('=').with_context(|| format!("no uStrong in {:?}", header[0]))?;
        let field =
            |i: usize| header.get(i).copied().unwrap_or_default().trim_matches('>').trim().to_string();
        let mut e = Entity {
            id: short_id(id),
            category,
            kind: field(8),
            description: String::new(),
            tribe: field(6),
            father: None,
            mother: None,
            siblings: Vec::new(),
            partners: Vec::new(),
            offspring: Vec::new(),
            founders: Vec::new(),
            inhabitants: Vec::new(),
            names: Vec::new(),
            locations: Vec::new(),
        };
        match category {
            Category::Person => {
                e.description = field(1);
                let parents = field(2);
                let mut parents = parents.split('+').map(str::trim);
                e.father = parents.next().filter(|s| !s.is_empty()).map(relation);
                e.mother = parents.next().filter(|s| !s.is_empty()).map(relation);
                e.siblings = relations(&field(3));
                e.partners = relations(&field(4));
                e.offspring = relations(&field(5));
            }
            Category::Place => {
                e.founders = relations(&field(2));
                e.inhabitants = relations(&field(3));
            }
            Category::Other => e.description = field(1),
        }
        for line in lines.filter(|l| l.starts_with("– ") && !l.starts_with("– Total")) {
            let f: Vec<&str> = line.split('\t').map(str::trim).collect();
            if f.len() < 5 {
                continue;
            }
            // "H0175«H0175=אַהֲרֹן", or "H0802J=." for someone mentioned but not named.
            let (numbers, original) = f[2].split_once('=').unwrap_or((f[2], ""));
            let strong = numbers.split('«').next().unwrap_or_default();
            let strong =
                original::normalize_strong(strong).with_context(|| format!("bad number in {line:?}"))?;
            let original = if original == "." { "" } else { original };
            let english = english_names(f[3]);
            let verses = verses(f[4], stats);
            // Variant lines repeat a form with more verses; they join it.
            match e.names.iter_mut().find(|n| n.strong == strong && n.original == original) {
                Some(n) => {
                    n.verses.extend(verses);
                    for name in english {
                        if !n.english.contains(&name) {
                            n.english.push(name);
                        }
                    }
                }
                None => e.names.push(Form { english, original: original.to_string(), strong, verses }),
            }
        }
        for n in &mut e.names {
            n.verses.sort_unstable();
            n.verses.dedup();
        }
        e.names.retain(|n| !n.verses.is_empty());
        if e.names.is_empty() {
            stats.empty.push(e.id.clone());
            continue;
        }
        openbible.push(if category == Category::Place { field(1) } else { String::new() });
        out.push(e);
    }
    Ok((out, openbible))
}

/// "Aaron@Exo.4.14-Heb" → "Aaron@Exo.4.14": the name and the reference that tells it apart.
fn short_id(id: &str) -> String {
    let id = id.trim();
    let Some((name, rest)) = id.split_once('@') else { return id.to_string() };
    let reference = rest.split(['-', '(', '=', ' ']).next().unwrap_or_default();
    format!("{name}@{reference}")
}

/// A relation as TIPNR writes it, with its marks in words: `(d)` on a group's forebear and `(a)`
/// on an ancestor make "(ancestor)", `(f)` "(founder)", and `(?)` an uncertain identification.
fn relation(s: &str) -> String {
    let s = s.trim();
    let mut out = short_id(s.split('|').next_back().unwrap_or(s));
    for (mark, word) in [("(d)", "ancestor"), ("(a)", "ancestor"), ("(f)", "founder"), ("(?)", "uncertain")] {
        if s.contains(mark) && !out.contains(word) {
            out.push_str(&format!(" ({word})"));
        }
    }
    out
}

fn relations(s: &str) -> Vec<String> {
    s.split([',', '+']).map(str::trim).filter(|r| r.contains('@')).map(relation).collect()
}

/// "Elias =KJV,TR; Elijah =ESV,NIV,NA" → ["Elias", "Elijah"]; `[ ]` (not in that translation)
/// is dropped.
fn english_names(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in s.split(';') {
        let name = part.split(" =").next().unwrap_or_default().trim();
        if !name.is_empty() && !name.starts_with('[') && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// "Exo.4.14; Exo.7.10a; Num.13.8,16; LXX.Gen.46.20" → verse keys on the KJV grid. A letter after
/// the verse numbers a second mention in it; Septuagint-only references are left out.
fn verses(s: &str, stats: &mut Stats) -> Vec<u32> {
    let mut out = Vec::new();
    let mut last: Option<(BookId, u8)> = None;
    for item in s.split([';', ',']).map(str::trim).filter(|t| !t.is_empty() && *t != "etc") {
        if item.starts_with("LXX") {
            stats.lxx += 1;
            continue;
        }
        let item = item.trim_end_matches("(?)").trim_end_matches(|c: char| c.is_ascii_lowercase());
        let parts: Vec<&str> = item.split('.').collect();
        let parsed = match parts[..] {
            [b, c, v] => BOOKS
                .iter()
                .position(|x| *x == b)
                .and_then(|i| Some((BookId::new(i)?, c.parse::<u8>().ok()?, v.parse::<u8>().ok()?))),
            // "Num.13.8,16": a verse in the same chapter.
            [v] => last.and_then(|(b, c)| Some((b, c, v.parse::<u8>().ok()?))),
            _ => None,
        };
        let Some((book, chapter, verse)) = parsed else {
            stats.unreadable.push(item.to_string());
            continue;
        };
        last = Some((book, chapter));
        let Some(c) = ChapterIdx::new(book, chapter) else {
            stats.unreadable.push(item.to_string());
            continue;
        };
        if verse > c.kjv_verses() {
            stats.clamped += 1;
        }
        out.push(VerseRef::new(book, chapter, verse.clamp(1, c.kjv_verses())).key());
    }
    out
}

/// One of OpenBible.info's ancient places: its name, the TIPNR ids it links to and its
/// identifications.
struct Place {
    name: String,
    tipnr: Vec<String>,
    locations: Vec<Location>,
    osm: usize,
}

fn geocoding(ancient: &str, modern: &str) -> Result<Vec<Place>> {
    // Modern locations whose coordinates come from OpenStreetMap (ODbL): the source says so, or the
    // point stands for OpenStreetMap geometry.
    let mut from_osm = HashSet::new();
    for line in modern.lines().filter(|l| !l.trim().is_empty()) {
        let m: Value = serde_json::from_str(line)?;
        let source = m["coordinates_source"]["type"].as_str().unwrap_or_default();
        let derived = source.is_empty() || source.starts_with("representative_point");
        if source.starts_with("osm") || derived && m["geometry_credit"] == "osm" {
            from_osm.insert(m["id"].as_str().unwrap_or_default().to_string());
        }
    }
    let mut out = Vec::new();
    for line in ancient.lines().filter(|l| !l.trim().is_empty()) {
        let a: Value = serde_json::from_str(line)?;
        let mut tipnr = Vec::new();
        for v in a["linked_data"].as_object().into_iter().flat_map(|o| o.values()) {
            let ids = v["ids"].as_array().into_iter().flatten().chain(std::iter::once(&v["id"]));
            tipnr.extend(ids.filter_map(Value::as_str).filter(|id| id.contains('@')).map(String::from));
        }
        let kind = a["types"][0].as_str().unwrap_or_default().to_string();
        let mut osm = 0;
        let mut locations: Vec<(i64, Location)> = Vec::new();
        for i in a["identifications"].as_array().into_iter().flatten() {
            let score = i["score"]["time_total"].as_i64().unwrap_or(0);
            if i["id_source"] == "special" || score <= 0 {
                continue;
            }
            let resolution = &i["resolutions"][0];
            let basis = resolution["modern_basis_id"].as_str();
            let from_osm = basis.is_none_or(|id| from_osm.contains(id));
            let lat_lon = resolution["lonlat"].as_str().filter(|_| !from_osm).and_then(|s| {
                let (lon, lat) = s.split_once(',')?;
                Some((lat.trim().parse().ok()?, lon.trim().parse().ok()?))
            });
            if from_osm && resolution["lonlat"].is_string() {
                osm += 1;
            }
            let identification = strip_tags(i["description"].as_str().unwrap_or_default());
            if identification.is_empty() {
                continue;
            }
            let kind = resolution["type"].as_str().map(String::from).unwrap_or_else(|| kind.clone());
            let confidence = (score / 10).clamp(1, 100) as u8;
            locations.push((score, Location { identification, kind, confidence, lat_lon }));
        }
        locations.sort_by_key(|l| std::cmp::Reverse(l.0));
        let locations: Vec<Location> = locations.into_iter().take(LOCATIONS).map(|(_, l)| l).collect();
        let name = a["friendly_id"].as_str().unwrap_or_default().to_string();
        out.push(Place { name, tipnr, locations, osm });
    }
    Ok(out)
}

/// Gives each TIPNR place OpenBible.info's identifications. TIPNR names the OpenBible.info place
/// it is ("Bethlehem_1" for "Bethlehem 1"); failing that, OpenBible.info names the TIPNR place.
fn attach_locations(
    entities: &mut [Entity],
    openbible_names: &[String],
    places: Vec<Place>,
    stats: &mut Stats,
) {
    let places: Vec<Place> = places.into_iter().filter(|p| !p.locations.is_empty()).collect();
    let by_name: HashMap<String, usize> =
        places.iter().enumerate().map(|(i, p)| (p.name.to_lowercase(), i)).collect();
    let by_id: HashMap<String, usize> = entities.iter().enumerate().map(|(i, e)| (key(&e.id), i)).collect();
    let mut place_of: Vec<Option<usize>> = vec![None; entities.len()];
    let mut used = vec![false; places.len()];
    // "Field of Blood= near Jerusalem": the name before any note; "Jericho" is "Jericho 1".
    for (i, name) in openbible_names.iter().enumerate() {
        let name = name.split('=').next().unwrap_or_default().replace('_', " ").trim().to_lowercase();
        if name.is_empty() {
            continue;
        }
        if let Some(&p) = by_name.get(&name).or_else(|| by_name.get(&format!("{name} 1"))) {
            place_of[i] = Some(p);
            used[p] = true;
            stats.matched_by_name += 1;
        }
    }
    for (p, place) in places.iter().enumerate().filter(|(p, _)| !used[*p]) {
        let mut linked = place.tipnr.iter().filter_map(|id| by_id.get(&key(id)).copied());
        match linked.find(|&i| entities[i].category == Category::Place && place_of[i].is_none()) {
            Some(i) => {
                place_of[i] = Some(p);
                stats.matched_by_id += 1;
            }
            None => stats.unmatched.push(place.name.clone()),
        }
    }
    for (e, p) in entities.iter_mut().zip(place_of) {
        if let Some(p) = p {
            e.locations = places[p].locations.clone();
            stats.osm += places[p].osm;
        }
    }
}

/// `along <modern id="m6dddbb">Wadi el Esh</modern>` → "along Wadi el Esh".
fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    linger_core::xml::unescape(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An id compared as `Names` compares them: without case or what follows the reference.
fn key(id: &str) -> String {
    short_id(id.split(" (").next().unwrap_or(id)).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_relations() {
        assert_eq!(short_id("Aaron@Exo.4.14-Heb"), "Aaron@Exo.4.14");
        assert_eq!(short_id("Agrippa@Act.25.13-"), "Agrippa@Act.25.13");
        assert_eq!(relation("Salma@1Ch.2.51-=H8007H"), "Salma@1Ch.2.51");
        assert_eq!(relation("Canaan@Gen.9.18-1Ch(d)"), "Canaan@Gen.9.18 (ancestor)");
        assert_eq!(relation("Jahdai@1Ch.2.47(?)"), "Jahdai@1Ch.2.47 (uncertain)");
        assert_eq!(relation("Tekoa@2Sa.14.2-Amo(f)"), "Tekoa@2Sa.14.2 (founder)");
        assert_eq!(relations("Nadab@Exo.6.23-1Ch, Abihu@Exo.6.23-1Ch"), ["Nadab@Exo.6.23", "Abihu@Exo.6.23"]);
        assert_eq!(english_names("Elias =KJV,TR; Elijah =ESV,NIV,NA"), ["Elias", "Elijah"]);
        assert!(english_names("[ ] =ESV").is_empty());
    }

    #[test]
    fn references_on_the_kjv_grid() {
        let mut stats = Stats::default();
        let keys = verses("Exo.7.10a; Exo.7.10b; Num.13.8,16; LXX.Gen.46.20; 3Jn.1.15; etc", &mut stats);
        let got: Vec<String> = keys.iter().map(|k| VerseRef::from_key(*k).unwrap().to_string()).collect();
        assert_eq!(got, ["Exodus 7:10", "Exodus 7:10", "Numbers 13:8", "Numbers 13:16", "3 John 1:14"]);
        assert_eq!((stats.lxx, stats.clamped), (1, 1));
    }

    #[test]
    fn location_text() {
        assert_eq!(strip_tags("along <modern id=\"m6dddbb\">Wadi el Esh</modern>"), "along Wadi el Esh");
    }
}
