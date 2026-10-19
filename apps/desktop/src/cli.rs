//! Command-line interface (headless): read, search, import and export without opening a window.
//!
//! ```text
//! linger read "jn 3:16" [--translation web]
//! linger search "love one another" [--translation kjv] [--scope nt|ot|<book>] [--limit 20]
//! linger list
//! linger import <file-or-folder> [--abbrev NASB] [--name "…"] [--key UNLOCK]
//! linger remove <id>
//! linger export --markdown|--json [-o file]
//! linger import-notes <export.json>
//! linger record [--source mic|system] [--seconds N]   (Enter stops)
//! linger ask "what does born of water mean?" [--at "jn 3:5"] [--translation kjv]
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use linger_core::library::Library;
use linger_core::model::Origin;
use linger_core::reference;
use linger_core::search::{self, Scope};
use linger_core::study::StudyStore;

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn positional(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") || a == "-o" {
            skip = !matches!(a.as_str(), "--json" | "--markdown");
            continue;
        }
        out.push(a);
    }
    out
}

/// Runs a CLI command if `args` names one; returns `None` to start the GUI.
pub fn run(args: &[String], data_dir: &Path) -> Option<Result<()>> {
    let cmd = args.first()?.as_str();
    let rest = &args[1..];
    Some(match cmd {
        "read" => read(rest, data_dir),
        "search" => search_cmd(rest, data_dir),
        "list" => list(data_dir),
        "import" => import(rest, data_dir),
        "remove" => remove(rest, data_dir),
        "export" => export(rest, data_dir),
        "import-notes" => import_notes(rest, data_dir),
        "record" => record(rest, data_dir),
        "ask" => ask(rest, data_dir),
        "--version" | "-V" => {
            println!("linger {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "--help" | "-h" | "help" => {
            println!(
                "{}",
                include_str!("cli.rs")
                    .lines()
                    .skip(3)
                    .take(12)
                    .map(|l| l.trim_start_matches("//! "))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            Ok(())
        }
        _ => return None,
    })
}

fn library(data_dir: &Path) -> Library {
    Library::open(data_dir, Arc::new(|| {}))
}

fn translation_id(rest: &[String], lib: &Library) -> String {
    flag(rest, "--translation")
        .or_else(|| flag(rest, "-t"))
        .filter(|id| lib.contains(id))
        .unwrap_or_else(|| "kjv".into())
}

fn read(rest: &[String], data_dir: &Path) -> Result<()> {
    let lib = library(data_dir);
    let id = translation_id(rest, &lib);
    let t = lib.load(&id)?;
    let input = positional(rest).into_iter().cloned().collect::<Vec<_>>().join(" ");
    let r = reference::parse(&input).with_context(|| format!("not a reference: {input:?}"))?;
    let c = r.chapter_idx();
    println!("{} ({})", r.display(), t.meta.abbrev);
    for v in t.verses_in(c) {
        let inside = match (r.verse, r.verse_end) {
            (None, _) => true,
            (Some(a), None) => v.covers(a),
            (Some(a), Some(b)) => v.number_end >= a && v.number <= b,
        };
        if inside {
            println!("{:>3}  {}", v.number, v.plain_text());
        }
    }
    Ok(())
}

trait Covers {
    fn covers(&self, n: u8) -> bool;
}

impl Covers for linger_core::model::VerseView<'_> {
    fn covers(&self, n: u8) -> bool {
        n >= self.number && n <= self.number_end
    }
}

fn search_cmd(rest: &[String], data_dir: &Path) -> Result<()> {
    let lib = library(data_dir);
    let id = translation_id(rest, &lib);
    let t = lib.load(&id)?;
    let query = positional(rest).into_iter().cloned().collect::<Vec<_>>().join(" ");
    let scope = match flag(rest, "--scope").as_deref() {
        None | Some("all") => Scope::All,
        Some("ot") => Scope::OldTestament,
        Some("nt") => Scope::NewTestament,
        Some(b) => Scope::Book(reference::parse(b).map(|r| r.book).context("unknown book")?),
    };
    let limit: usize = flag(rest, "--limit").and_then(|l| l.parse().ok()).unwrap_or(20);
    // Build the index first so the timing reflects the query itself.
    let built = std::time::Instant::now();
    t.search_index();
    let index_ms = built.elapsed().as_secs_f64() * 1000.0;
    let out = search::run(&t, &query, scope, rest.iter().any(|a| a == "--whole-words"));
    println!(
        "{} verses in {:.3} ms ({}; index built in {:.1} ms)",
        out.verses.len(),
        out.elapsed.as_secs_f64() * 1000.0,
        t.meta.abbrev,
        index_ms
    );
    for &i in out.verses.iter().take(limit) {
        let v = t.verse_view(i).unwrap();
        println!("{:<22} {}", v.reference().to_string(), v.plain_text());
    }
    Ok(())
}

fn list(data_dir: &Path) -> Result<()> {
    let lib = library(data_dir);
    for e in lib.entries() {
        let m = &e.meta;
        let origin = match &m.origin {
            Origin::Bundled => "bundled".to_string(),
            Origin::Imported { format, .. } => format!("imported {format}"),
            Origin::Online { provider, .. } => format!("online ({provider})"),
        };
        println!("{:<10} {:<6} {:<34} {:<18} {}", m.id, m.abbrev, m.name, origin, m.license);
    }
    Ok(())
}

fn import(rest: &[String], data_dir: &Path) -> Result<()> {
    let pos = positional(rest);
    let path = PathBuf::from(pos.first().context("usage: linger import <file-or-folder>")?);
    let opts = linger_core::import::ImportOptions {
        id: flag(rest, "--id"),
        abbrev: flag(rest, "--abbrev"),
        name: flag(rest, "--name"),
        unlock_key: flag(rest, "--key"),
    };
    let started = std::time::Instant::now();
    let imported = linger_core::import::import_path(&path, &opts)?;
    for w in imported.warnings.iter().take(11) {
        eprintln!("warning: {w}");
    }
    let format = imported.format.name();
    let verses = imported.translation.verse_count();
    let name = imported.translation.meta.name.clone();
    let mut lib = library(data_dir);
    let id = lib.install(imported)?;
    println!(
        "Imported {name} ({format}, {verses} verses) as \"{id}\" in {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    println!("Stored in {}", linger_core::import::imports_dir(data_dir).display());
    Ok(())
}

fn remove(rest: &[String], data_dir: &Path) -> Result<()> {
    let id = positional(rest).first().map(|s| s.to_string()).context("usage: linger remove <id>")?;
    library(data_dir).remove(&id)?;
    println!("Removed {id}");
    Ok(())
}

fn export(rest: &[String], data_dir: &Path) -> Result<()> {
    let study = StudyStore::open(&data_dir.join("study.sqlite"))?;
    let lib = library(data_dir);
    let id = translation_id(rest, &lib);
    let t = lib.load(&id)?;
    let text = |v| t.find(v).map(|v| v.plain_text());
    let body = if rest.iter().any(|a| a == "--json") {
        study.export_json(&t.meta.abbrev, text)?
    } else if rest.iter().any(|a| a == "--markdown") {
        study.export_markdown(&t.meta.abbrev, text)
    } else {
        bail!("usage: linger export --markdown|--json [-o file]");
    };
    match flag(rest, "-o") {
        Some(path) => std::fs::write(&path, body).with_context(|| path.clone())?,
        None => print!("{body}"),
    }
    Ok(())
}

fn import_notes(rest: &[String], data_dir: &Path) -> Result<()> {
    let path = positional(rest)
        .first()
        .map(|s| s.to_string())
        .context("usage: linger import-notes <export.json>")?;
    let mut study = StudyStore::open(&data_dir.join("study.sqlite"))?;
    let (h, n, b) = study.import_json(&std::fs::read_to_string(&path).with_context(|| path.clone())?)?;
    println!("Imported {h} highlights, {n} notes and {b} bookmarks");
    Ok(())
}

/// One question to the assistant, with the answer streamed to stdout and its lookups to stderr.
fn ask(rest: &[String], data_dir: &Path) -> Result<()> {
    use std::io::Write;

    use linger_core::assistant::{Config, Conversation, Event};
    use linger_core::chat::{Context as Reading, Tools};
    let cfg = Config::from_env().context("set AI_GATEWAY_API_KEY to ask the assistant")?;
    let lib = library(data_dir);
    let tools = Tools { translation: lib.load(&translation_id(rest, &lib))? };
    let question = positional(rest).into_iter().cloned().collect::<Vec<_>>().join(" ");
    if question.trim().is_empty() {
        bail!("usage: linger ask \"question\" [--at \"jn 3:5\"]");
    }
    let context = flag(rest, "--at")
        .map(|at| -> Result<Reading> {
            let r = reference::parse(&at).context("--at: not a reference")?;
            let selection = r.verse.map(|v| {
                let a = linger_core::VerseRef::new(r.book, r.chapter, v);
                (a, linger_core::VerseRef::new(r.book, r.chapter, r.verse_end.unwrap_or(v)))
            });
            Ok(Reading { chapter: r.chapter_idx(), selection })
        })
        .transpose()?;
    let answer = Conversation::new().ask(&cfg, &tools, context.as_ref(), &question, |e| match e {
        Event::Text(t) => {
            print!("{t}");
            let _ = std::io::stdout().flush();
        }
        Event::Tool(what) => eprintln!("\n[{what}]"),
        Event::Thinking => eprintln!("\n[thinking]"),
    })?;
    println!();
    eprintln!("\n{} verses cited", answer.cited().len());
    Ok(())
}

fn record(rest: &[String], data_dir: &Path) -> Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};

    use linger_core::recording::Source;
    let source = match flag(rest, "--source").as_deref() {
        None | Some("mic") => Source::Microphone,
        Some("system") => Source::SystemAudio,
        Some(other) => bail!("--source is mic or system, not {other:?}"),
    };
    let seconds: Option<u64> = flag(rest, "--seconds").map(|s| s.parse()).transpose().context("--seconds")?;
    let rec = crate::recorder::Recording::start(data_dir, source)?;
    eprintln!("Recording {} (press Enter to stop)", rec.id());
    // Only a line on stdin stops; a closed stdin (no terminal) doesn't.
    let enter = Arc::new(AtomicBool::new(false));
    {
        let enter = enter.clone();
        std::thread::spawn(move || {
            if std::io::stdin().read_line(&mut String::new()).is_ok_and(|n| n > 0) {
                enter.store(true, Ordering::Relaxed);
            }
        });
    }
    while !enter.load(Ordering::Relaxed)
        && !rec.failed()
        && seconds.is_none_or(|s| rec.elapsed().as_secs() < s)
    {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let bars = (rec.level() * 4.0).sqrt().min(1.0) * 30.0;
        eprint!("\r{:>5.1}s  {:<30}", rec.elapsed().as_secs_f32(), "▮".repeat(bars as usize));
        let _ = std::io::stderr().flush();
    }
    let m = rec.stop()?;
    eprintln!();
    println!(
        "{}  {:.1}s in {} chunk(s)  {}",
        m.id,
        m.duration_ms() as f64 / 1000.0,
        m.chunks.len(),
        linger_core::recording::recordings_dir(data_dir).join(&m.id).display()
    );
    Ok(())
}
