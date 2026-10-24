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
//! linger record [--source mic|system] [--seconds N] [--timeout S]   (Enter stops; uploads as it records when the library syncs)
//! linger ask "what does born of water mean?" [--at "jn 3:5"] [--translation kjv] [--deep]
//! linger signin | signout [--force] | account    (Linger's account, for Ask and cloud transcripts)
//! linger sync [--wait] [--timeout 120]   (sends local changes, pulls the library; prints the cursor)
//! linger sermons
//! linger sermon <recording-id> [--title "…"] [--preacher "…"] [--from "…"]   (shows or sets details)
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
        "signin" => signin(data_dir),
        "signout" => signout(rest, data_dir),
        "sync" => sync(rest, data_dir),
        "account" => account(data_dir),
        "sermons" => sermons(data_dir),
        "sermon" => sermon(rest, data_dir),
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
                    .take(16)
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
    use linger_core::chat::{Context as Reading, Tools, ask_translation};
    let mode = if rest.iter().any(|a| a == "--deep") {
        linger_core::assistant::Mode::Deep
    } else {
        linger_core::assistant::Mode::Standard
    };
    // Signed in, through Linger's proxy; otherwise straight to the Gateway with a developer key.
    let cfg = match signed_in(data_dir) {
        Some(api) => Config::proxy(api, mode),
        None => {
            Config::from_env().context("sign in with `linger signin` (or set AI_GATEWAY_API_KEY) to ask")?
        }
    };
    let lib = library(data_dir);
    // The tools read a public-domain translation: the one named with --translation if it's one
    // of those, else the BSB. Only the name of any other reaches the model.
    let reading = translation_id(rest, &lib);
    let reading_abbrev = lib.get(&reading).map_or_else(|| reading.to_uppercase(), |e| e.meta.abbrev.clone());
    let asked = flag(rest, "--translation").or_else(|| flag(rest, "-t")).unwrap_or_default();
    let tools = Tools {
        translation: lib.load(ask_translation(&asked))?,
        // The CLI has no sync engine, so no library: the desktop app's Ask has.
        library: None,
        church: StudyStore::open(&data_dir.join("study.sqlite"))
            .ok()
            .and_then(|s| crate::app::Settings::load(&s).home_church),
    };
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
            Ok(Reading { chapter: r.chapter_idx(), selection, reading: reading_abbrev.clone(), item: None })
        })
        .transpose()?;
    let answer = Conversation::new().ask(&cfg, &tools, context.as_ref(), &question, |e| match e {
        Event::Text(t) => {
            print!("{t}");
            let _ = std::io::stdout().flush();
        }
        Event::Tool(what) => eprintln!("\n[{what}]"),
        Event::Thinking => eprintln!("\n[thinking]"),
        Event::Read(_) => {}
        Event::Allowance { questions_left, downgraded } => {
            if let Some(n) = questions_left {
                eprintln!("[{n} questions left this month]");
            }
            if downgraded {
                eprintln!("[past this month's limit for the full model: answered by a lighter one]");
            }
        }
    })?;
    println!();
    eprintln!("\n{} verses cited", answer.cited().len());
    Ok(())
}

/// The proxy client, when this device is signed in.
fn signed_in(data_dir: &Path) -> Option<linger_core::api::Api> {
    let store = linger_core::secrets::SecretStore::new(data_dir);
    linger_core::auth::saved(&store).map(|t| linger_core::api::Api::new(linger_core::api::base_url(), t))
}

/// Browser sign-in (OAuth with PKCE through Clerk, loopback redirect).
fn signin(data_dir: &Path) -> Result<()> {
    use linger_core::auth::{OAuthConfig, OAuthSession};
    let store = linger_core::secrets::SecretStore::new(data_dir);
    OAuthSession::sign_in(OAuthConfig::from_env(), store, |url| {
        eprintln!("Opening the browser to sign in. If it doesn't open, visit:\n{url}");
        crate::app::open_url(url);
    })?;
    eprintln!("Signed in.");
    account(data_dir)
}

/// Signs out and forgets the library's cache; refuses while changes are waiting, unless `--force`.
fn signout(rest: &[String], data_dir: &Path) -> Result<()> {
    let mut study = StudyStore::open(&data_dir.join("study.sqlite"))?;
    let waiting = study.pending_changes();
    if waiting > 0 && !rest.iter().any(|a| a == "--force") {
        bail!(
            "{waiting} changes haven't reached your library yet; run `linger sync --wait` first, or \
             `linger signout --force` to lose them"
        );
    }
    study.clear_library()?;
    let store = linger_core::secrets::SecretStore::new(data_dir);
    if let Err(e) = linger_core::auth::sign_out(&store) {
        eprintln!("Signed out on this device ({e:#}).");
    } else {
        eprintln!("Signed out.");
    }
    Ok(())
}

/// Sends what is waiting and pulls the library, then prints the feed's cursor. `--wait` is the
/// only mode: it returns once the outbox is empty and the feed is caught up, or fails at the
/// timeout with the engine's status.
fn sync(rest: &[String], data_dir: &Path) -> Result<()> {
    use linger_core::sync::{self, ConvexBackend};
    let store = linger_core::secrets::SecretStore::new(data_dir);
    let tokens = linger_core::auth::saved(&store).context("not signed in; run `linger signin`")?;
    let url = linger_core::api::convex_url();
    // Conflicts are a log kept for the UI; this run reports only the ones it records.
    let before = StudyStore::open(&data_dir.join("study.sqlite"))?.conflicts().len();
    let handle = sync::start(data_dir, Box::new(ConvexBackend::new(&url)?), tokens)?;
    let secs =
        flag(rest, "--timeout").map(|s| s.parse()).transpose().context("--timeout <seconds>")?.unwrap_or(120);
    let cursor = handle.wait_idle(std::time::Duration::from_secs(secs)).map_err(|status| match status {
        sync::SyncStatus::Error(e) => anyhow::anyhow!("not synced: {e}"),
        status => anyhow::anyhow!("not synced after {secs} s: {status:?}"),
    })?;
    let study = StudyStore::open(&data_dir.join("study.sqlite"))?;
    let (h, _, b) = study.counts();
    println!("synced with {url}: cursor {cursor}, {h} highlights, {b} bookmarks");
    let conflicts = study.conflicts();
    // conflicts() is newest first, so this run's are the head of the list.
    for c in conflicts.iter().take(conflicts.len().saturating_sub(before)) {
        if c.kind == "note" {
            // The edit isn't lost: the server kept it as a copy beside the newer note.
            println!(
                "conflict on note {}: kept {} and this device's {} as a copy",
                c.verse, c.theirs, c.mine
            );
        } else {
            println!("conflict on {} {}: kept {} over this device's {}", c.kind, c.verse, c.theirs, c.mine);
        }
    }
    Ok(())
}

/// The study store with a running sync engine, when this device is signed in and the library
/// consent sheet was accepted (as the app starts sync); `None` keeps the recording on disk, and
/// the app queues it when it next syncs.
fn upload_library(data_dir: &Path) -> Result<Option<(StudyStore, linger_core::sync::SyncHandle)>> {
    use linger_core::sync::{self, ConvexBackend};
    let mut study = StudyStore::open(&data_dir.join("study.sqlite"))?;
    // Consent first: without it the sign-in (and the Keychain) isn't touched.
    if crate::app::Settings::load(&study).library_consent != Some(true) {
        return Ok(None);
    }
    let store = linger_core::secrets::SecretStore::new(data_dir);
    let Some(tokens) = linger_core::auth::saved(&store) else { return Ok(None) };
    let handle =
        sync::start(data_dir, Box::new(ConvexBackend::new(&linger_core::api::convex_url())?), tokens)?;
    study.on_outbox(handle.notifier());
    Ok(Some((study, handle)))
}

/// "04:31:07": the local time, for the record command's log.
fn stamp() -> String {
    let l = crate::time::local(linger_core::study::now());
    let s = linger_core::study::now().rem_euclid(60);
    format!("{:02}:{:02}:{s:02}", l.hour, l.minute)
}

/// The plan and what's left of it.
fn account(data_dir: &Path) -> Result<()> {
    let api = signed_in(data_dir).context("not signed in; run `linger signin`")?;
    let me = api.me()?;
    println!(
        "{} ({})",
        me.user.email.as_deref().unwrap_or(&me.user.id),
        if me.is_pro() { "Pro" } else { "Free" }
    );
    println!("Ask: {} questions left, resets {}", me.ask.questions_left, me.ask.resets_at);
    if let Some(l) = &me.library {
        let mb = |b: u64| b as f64 / 1e6;
        println!("Library: {:.1} MB of {:.0} MB used", mb(l.storage_used), mb(l.storage_limit));
    }
    if me.is_pro() {
        println!("Sermons: {:.1} hours of cloud transcripts left", me.sermons.hours_left);
    } else if me.sermons.trial_available {
        println!("Sermons: one free cloud trial hour available");
    }
    Ok(())
}

fn sermons(data_dir: &Path) -> Result<()> {
    use linger_core::transcribe;
    for t in transcribe::all(data_dir) {
        let words: usize = t.text.segments.iter().map(|s| s.text.split_whitespace().count()).sum();
        let d = &t.about.details;
        let by: Vec<&str> = [d.speaker.as_deref(), d.from.as_deref()].into_iter().flatten().collect();
        let by = if by.is_empty() { String::new() } else { format!("  ({})", by.join(", ")) };
        println!(
            "{}  {}{by}  {}  {words} words",
            t.recording_id,
            t.title(),
            transcribe::clock(t.text.duration_ms)
        );
    }
    Ok(())
}

/// Shows a recording's details, or sets those given.
fn sermon(rest: &[String], data_dir: &Path) -> Result<()> {
    use linger_core::recording;
    let id = positional(rest).first().map(|s| s.as_str()).context("usage: linger sermon <recording-id>")?;
    let m = recording::list(data_dir)
        .into_iter()
        .find(|m| m.id.starts_with(id))
        .context("no recording with that id")?;
    let mut d = m.details.clone();
    let mut changed = false;
    for (name, field) in [("--title", &mut d.title), ("--preacher", &mut d.speaker), ("--from", &mut d.from)]
    {
        if let Some(v) = flag(rest, name) {
            *field = Some(v);
            changed = true;
        }
    }
    let m = if changed { recording::save_details(data_dir, &m.id, d)? } else { m };
    let d = &m.details;
    println!("{}", m.id);
    for (label, v) in
        [("Title", &d.title), ("Preacher", &d.speaker), ("From", &d.from), ("Played in", &d.app)]
    {
        println!("  {label}: {}", v.as_deref().unwrap_or("—"));
    }
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
    // Signed in, with the library syncing: the recording goes to the library as it records, as
    // in the app. Started first, so an engine that can't start leaves no recording behind.
    let mut library = upload_library(data_dir)?;
    let rec = crate::recorder::Recording::start(data_dir, source)?;
    eprintln!("Recording {} (press Enter to stop)", rec.id());
    if let Some((study, sync)) = &mut library {
        while !sync.claimed() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        study.queue_recordings(data_dir, Some(rec.id()))?;
    }
    let dir = rec.dir(data_dir);
    let (mut queued_parts, mut pending) = (None, 0);
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
        if let Some((study, _)) = &mut library {
            let parts = rec.parts();
            if queued_parts != Some(parts)
                && let Some(m) = linger_core::recording::read(&dir)
            {
                queued_parts = Some(parts);
                study.queue_recording(&dir, &m)?;
            }
            let now = study.pending_changes();
            if now != pending {
                pending = now;
                eprintln!(
                    "\r{}  {:>6.1}s  {parts} part(s) closed, {now} change(s) waiting",
                    stamp(),
                    rec.elapsed().as_secs_f32()
                );
            }
        }
        let bars = (rec.level() * 4.0).sqrt().min(1.0) * 30.0;
        let meter =
            if rec.waiting() { "waiting for permission".to_owned() } else { "▮".repeat(bars as usize) };
        eprint!("\r{:>5.1}s  {meter:<30}", rec.elapsed().as_secs_f32());
        let _ = std::io::stderr().flush();
    }
    let m = rec.stop()?;
    eprintln!();
    if let Some((study, sync)) = &mut library {
        study.queue_recording(&dir, &m)?;
        // Like signing out, the run waits until the library has it all.
        let secs: u64 = flag(rest, "--timeout")
            .map(|s| s.parse())
            .transpose()
            .context("--timeout <seconds>")?
            .unwrap_or(600);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        while study.pending_changes() > 0 && std::time::Instant::now() < deadline {
            let now = study.pending_changes();
            if now != pending {
                pending = now;
                eprintln!("{}  stopped, {now} change(s) waiting", stamp());
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        match study.pending_changes() {
            0 => eprintln!("{}  in your library ({:?})", stamp(), sync.status()),
            n => eprintln!(
                "{n} change(s) still waiting after {secs} s ({:?}); they go at the next sync",
                sync.status()
            ),
        }
    }
    println!(
        "{}  {:.1}s in {} chunk(s)  {}",
        m.id,
        m.duration_ms() as f64 / 1000.0,
        m.chunks.len(),
        linger_core::recording::recordings_dir(data_dir).join(&m.id).display()
    );
    Ok(())
}
