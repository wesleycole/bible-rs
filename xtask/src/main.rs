//! `cargo run -p xtask -- build-data`
//!
//! `cargo xtask build-original` writes `data/original/original.zst`, the Hebrew and Greek text
//! and lexicon for Ask's word-study tools (see `original.rs`).
//!
//! `cargo xtask build-crossrefs` writes `data/crossrefs/crossrefs.zst` and `cargo xtask build-names`
//! writes `data/names/names.zst`, the cross-references and the people and places for Ask (see
//! `crossrefs.rs` and `names.rs`).
//!
//! `cargo xtask contract` writes `server/src/contract/ask-v<N>.json` from linger-core (see
//! `linger_core::contract`).
//!
//! `cargo xtask schema` writes `server/shared/records.ts` and `server/convex/lib/canon.json` from
//! linger-core's record types (see `linger_core::records`).
//!
//! Downloads the public-domain USFM sources pinned below from eBible.org (into `.sources/`),
//! verifies their SHA-256, converts them with the same USFM importer users get, and writes
//! `data/translations/<id>.bbl`. Set `XTASK_ACCEPT_UPDATED_SOURCES=1` to accept a newer upstream
//! file (its new hash is printed so the pin can be updated).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use linger_core::format;
use linger_core::model::{Origin, TranslationBuilder, TranslationMeta};
use linger_core::usfm;
use sha2::{Digest, Sha256};

mod crossrefs;
mod names;
mod original;
mod sources;

struct Source {
    id: &'static str,
    abbrev: &'static str,
    name: &'static str,
    ebible_id: &'static str,
    sha256: &'static str,
    license: &'static str,
    copyright: &'static str,
}

const SOURCES: &[Source] = &[
    Source {
        id: "kjv",
        abbrev: "KJV",
        name: "King James Version",
        ebible_id: "eng-kjv2006",
        sha256: "77bbe9c78f4f588b1465e49ddad986293a09e7f4b140abe7248232c8c5ae2a5b",
        license: "Public Domain (outside the United Kingdom)",
        copyright: "King James Version (1769 standardized text). Public domain outside the United Kingdom, where \
                    the Crown's letters patent apply.",
    },
    Source {
        id: "web",
        abbrev: "WEB",
        name: "World English Bible",
        ebible_id: "engwebp",
        sha256: "6b47e6553ce0bd71902fe4b4ba71a5d7d862928e2ce47936acdda345bc6c5dd0",
        license: "Public Domain",
        copyright: "The World English Bible is in the public domain. \u{201c}World English Bible\u{201d} is a \
                    trademark of eBible.org.",
    },
    Source {
        id: "asv",
        abbrev: "ASV",
        name: "American Standard Version",
        ebible_id: "eng-asv",
        sha256: "161ac2da2b0668cd7c6d3a2bffbf13aeb5c58b9a0ba3bfab0f576b232cb4adc8",
        license: "Public Domain",
        copyright: "American Standard Version (1901). Public domain.",
    },
    Source {
        id: "bsb",
        abbrev: "BSB",
        name: "Berean Standard Bible",
        ebible_id: "engbsb",
        sha256: "872c04668a29fe674c0013b6ebcbbcc81bc30dd0d9ae9f6e85bc53489749b37d",
        license: "Public Domain (dedicated April 30, 2023)",
        copyright: "The Holy Bible, Berean Standard Bible, BSB is produced in cooperation with Bible Hub, \
                    Discovery Bible, OpenBible.com, and the Berean Bible Translation Committee. This text of \
                    God's Word has been dedicated to the public domain.",
    },
    Source {
        id: "ylt",
        abbrev: "YLT",
        name: "Young's Literal Translation",
        ebible_id: "engylt",
        sha256: "987b38d903546e16af66caa69cab40c5abc1959db9ea64168531a497abc59d5c",
        license: "Public Domain",
        copyright: "Young's Literal Translation (Robert Young, 1898). Public domain.",
    },
    Source {
        id: "dby",
        abbrev: "DBY",
        name: "Darby Translation",
        ebible_id: "engDBY",
        sha256: "ebb1952e7ee657e6e45065178257009185ba89230de3c59d88e740ac60fd1a70",
        license: "Public Domain",
        copyright: "The Holy Scriptures, a New Translation from the Original Languages by J. N. Darby (1890). \
                    Public domain.",
    },
    Source {
        id: "wbs",
        abbrev: "WBS",
        name: "Webster Bible",
        ebible_id: "engwebster",
        sha256: "16748b10c224e0ec0acdf183b76741ba618e3b8f1c3a15f1a6464c04331a6b78",
        license: "Public Domain",
        copyright: "The Holy Bible, Containing the Old and New Testaments, in the Common Version, with \
                    Amendments of the Language by Noah Webster, LL.D. (1833). Public domain.",
    },
];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    match args.first().map(String::as_str) {
        Some("build-data") => build_data(&root, &args[1..]),
        Some("build-original") => original::build(&root),
        Some("build-crossrefs") => crossrefs::build(&root),
        Some("build-names") => names::build(&root),
        Some("build-sources") => sources::build(&root, &args[1..]),
        Some("contract") => {
            // The Ask contract the proxy injects, generated from linger-core's prompt and tools.
            let path = root.join("server/src/contract").join(linger_core::contract::file_name());
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, linger_core::contract::file())?;
            println!(
                "wrote {} ({} {})",
                path.display(),
                linger_core::contract::VERSION,
                linger_core::contract::sha256()
            );
            Ok(())
        }
        Some("schema") => {
            // The synced records and the canonical grid, generated from linger-core's types so the
            // Convex validators, the Effect schemas and the apps can't drift apart.
            let ts = root.join("server/shared/records.ts");
            std::fs::create_dir_all(ts.parent().unwrap())?;
            std::fs::write(&ts, linger_core::records::records_ts())?;
            let canon = root.join("server/convex/lib/canon.json");
            std::fs::write(&canon, linger_core::records::canon_json())?;
            println!("wrote {} and {}", ts.display(), canon.display());
            Ok(())
        }
        Some("missing") => {
            // Lists verses present in the first translation but absent from the others.
            let load = |id: &str| -> Result<linger_core::Translation> {
                format::decode(&std::fs::read(root.join(format!("data/translations/{id}.bbl")))?)
            };
            let base = load(&args[1])?;
            for other in &args[2..] {
                let t = load(other)?;
                let mut missing = Vec::new();
                for c in linger_core::ChapterIdx::all() {
                    for v in base.verses_in(c) {
                        if t.find(v.reference()).is_none() {
                            missing.push(v.reference().to_string());
                        }
                    }
                }
                println!("{other}: {} missing: {}", missing.len(), missing.join(", "));
            }
            Ok(())
        }
        Some("dump") => {
            let t = format::decode(&std::fs::read(root.join(format!("data/translations/{}.bbl", args[1])))?)?;
            let r = linger_core::reference::parse(&args[2]).context("bad reference")?;
            let c = r.chapter_idx();
            println!("{} — {:?}", c.display(), t.meta.books[c.book().index()]);
            for v in t.verses_in(c) {
                let marks: Vec<String> = v
                    .marks
                    .iter()
                    .map(|m| {
                        format!(
                            "{:?}{}@{}{}",
                            m.kind,
                            m.level,
                            m.offset,
                            if m.kind.has_text() {
                                format!("“{}”", t.mark_text(m))
                            } else {
                                String::new()
                            }
                        )
                    })
                    .collect();
                let spans: Vec<String> =
                    v.spans.iter().map(|s| format!("{:?}[{}..{}]", s.style, s.start, s.end)).collect();
                println!("{:>3} {}\n      marks {:?} spans {:?}", v.number, v.text, marks, spans);
            }
            Ok(())
        }
        Some("verse-table") => {
            let t = format::decode(&std::fs::read(root.join("data/translations/kjv.bbl"))?)?;
            let counts: Vec<String> =
                linger_core::ChapterIdx::all().map(|c| t.last_verse(c).to_string()).collect();
            for line in counts.chunks(30) {
                println!("    {},", line.join(", "));
            }
            Ok(())
        }
        Some("import") => {
            // Imports a file and compares it verse by verse with a bundled translation.
            let started = std::time::Instant::now();
            let imp = linger_core::import::import_path(Path::new(&args[1]), &Default::default())?;
            let t = imp.translation;
            println!(
                "{} — {} ({}) — {} verses in {:?}; {} warnings",
                imp.format.name(),
                t.meta.name,
                t.meta.abbrev,
                t.verse_count(),
                started.elapsed(),
                imp.warnings.len()
            );
            if let Some(base) = args.get(2) {
                let b = format::decode(&std::fs::read(root.join(format!("data/translations/{base}.bbl")))?)?;
                let (mut same, mut diff, mut missing) = (0, 0, 0);
                for c in linger_core::ChapterIdx::all() {
                    for v in b.verses_in(c) {
                        match t.find(v.reference()) {
                            Some(o) if o.plain_text() == v.plain_text() => same += 1,
                            Some(o) => {
                                if diff < 5 {
                                    println!(
                                        "  {}\n    {}\n    {}",
                                        v.reference(),
                                        v.plain_text(),
                                        o.plain_text()
                                    );
                                }
                                diff += 1
                            }
                            None => missing += 1,
                        }
                    }
                }
                println!("vs {base}: {same} identical, {diff} different, {missing} missing");
            }
            for r in ["ps 23:1", "jn 3:16", "gen 1:1", "rev 22:21"] {
                let r = linger_core::reference::parse(r).unwrap();
                if let Some(v) = t.find(r.verse_ref()) {
                    let marks: Vec<String> =
                        v.marks.iter().map(|m| format!("{:?}{}", m.kind, m.level)).collect();
                    let spans: Vec<String> = v.spans.iter().map(|s| format!("{:?}", s.style)).collect();
                    println!("{}: {} {marks:?} {spans:?}", r.display(), v.text);
                }
            }
            Ok(())
        }
        Some("fetch") => {
            for s in selected(&args[1..]) {
                fetch(&root, s)?;
            }
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: cargo run -p xtask -- build-data [ids…] | build-original | build-crossrefs | build-names | build-sources [keys…] | fetch [ids…] | contract | schema"
            );
            std::process::exit(2)
        }
    }
}

fn selected(ids: &[String]) -> Vec<&'static Source> {
    SOURCES.iter().filter(|s| ids.is_empty() || ids.iter().any(|i| i == s.id)).collect()
}

fn fetch(root: &Path, s: &Source) -> Result<PathBuf> {
    let url = format!("https://ebible.org/Scriptures/{}_usfm.zip", s.ebible_id);
    fetch_pinned(root, &format!("{}_usfm.zip", s.ebible_id), &url, s.sha256)
}

/// Downloads `url` into `.sources/<name>` unless it is there, and checks its SHA-256. For a source
/// that changes upstream without a version (eBible.org's archives, OpenBible.info's data),
/// `XTASK_ACCEPT_UPDATED_SOURCES=1` accepts a new file and prints its hash for the pin.
fn fetch_pinned(root: &Path, name: &str, url: &str, sha256: &str) -> Result<PathBuf> {
    let dir = root.join(".sources");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(name);
    if !file.exists() {
        eprintln!("downloading {url}");
        let status = Command::new("curl").args(["-fsSL", "-o"]).arg(&file).arg(url).status()?;
        if !status.success() {
            bail!("download failed: {url}");
        }
    }
    let digest = hex(&Sha256::digest(std::fs::read(&file)?));
    if digest != sha256 {
        if std::env::var_os("XTASK_ACCEPT_UPDATED_SOURCES").is_some() {
            eprintln!("warning: {name} changed upstream; new sha256 = {digest}");
        } else {
            bail!(
                "{} sha256 mismatch: expected {sha256}, got {digest}. The upstream file changed; review it, then \
                 update the pin (or set XTASK_ACCEPT_UPDATED_SOURCES=1).",
                file.display(),
            );
        }
    }
    Ok(file)
}

fn build_data(root: &Path, ids: &[String]) -> Result<()> {
    let out_dir = root.join("data/translations");
    std::fs::create_dir_all(&out_dir)?;
    for s in selected(ids) {
        let zip_path = fetch(root, s)?;
        let digest = hex(&Sha256::digest(std::fs::read(&zip_path)?));
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path)?)?;
        let mut names: Vec<String> =
            archive.file_names().filter(|n| n.ends_with(".usfm")).map(String::from).collect();
        names.sort();
        let meta = TranslationMeta {
            id: s.id.into(),
            abbrev: s.abbrev.into(),
            name: s.name.into(),
            language: "en".into(),
            license: s.license.into(),
            copyright: s.copyright.into(),
            source: format!(
                "https://ebible.org/Scriptures/{}_usfm.zip (sha256 {digest}); details: https://ebible.org/{}/",
                s.ebible_id, s.ebible_id
            ),
            origin: Origin::Bundled,
            ..Default::default()
        };
        let mut b = TranslationBuilder::new(meta);
        for name in &names {
            let mut f = archive.by_name(name)?;
            let mut text = String::new();
            std::io::Read::read_to_string(&mut f, &mut text).with_context(|| name.clone())?;
            usfm::UsfmParser::new(&mut b).parse(&text)?;
        }
        let warnings = std::mem::take(&mut b.warnings);
        let t = b.finish();
        let bytes = format::encode(&t, 19)?;
        let path = out_dir.join(format!("{}.bbl", s.id));
        std::fs::write(&path, &bytes)?;
        println!(
            "{:<4} {:>6} verses  {:>5.2} MB text → {:>5.2} MB  ({} warnings){}",
            s.abbrev,
            t.verse_count(),
            t.total_text_bytes() as f64 / 1e6,
            bytes.len() as f64 / 1e6,
            warnings.len(),
            if t.meta.red_letters { "  red letters" } else { "" }
        );
        for w in warnings.iter().take(5) {
            println!("      {w}");
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
