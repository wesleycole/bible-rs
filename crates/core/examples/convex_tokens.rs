//! The M1 tokens spike, against a real Convex deployment with real Clerk tokens.
//!
//! 1. A desktop OAuth access token and a Clerk session token for the same person resolve to one
//!    `tokenIdentifier` (`library/whoami:get`).
//! 2. `set_auth_callback` is asked for a token again when the socket reconnects, including the
//!    reconnect Convex forces when a session token (60 s) expires.
//! 3. `set_auth` can replace a token before it expires without disturbing a subscription.
//! 4. What a token past its `exp` still does on an open socket, with no refresh.
//!
//! ```sh
//! LINGER_CONVEX_URL=https://<dev>.convex.cloud cargo run -p linger-core --features convex \
//!   --example convex_tokens -- "pnpm -s --dir server clerk-test-token --oauth" \
//!   "pnpm -s --dir server clerk-test-token --session"
//! ```
//!
//! Each argument is a shell command that prints a fresh token.

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use convex::{AuthTokenFetcher, AuthenticationToken, ConvexClient, FunctionResult, Value};
use futures::StreamExt;

fn token(cmd: &str) -> Result<String> {
    let out = Command::new("sh").arg("-c").arg(cmd).output()?;
    if !out.status.success() {
        bail!("{cmd}: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

fn show(r: &FunctionResult) -> String {
    match r {
        FunctionResult::Value(v) => serde_json::Value::from(v.clone()).to_string(),
        FunctionResult::ErrorMessage(m) => format!("error: {m}"),
        FunctionResult::ConvexError(e) => format!("ConvexError: {e:?}"),
    }
}

/// The identity a token resolves to, after a no-op write that makes sure the `users` row exists.
async fn identity(url: &str, token: String) -> Result<serde_json::Value> {
    let mut client = ConvexClient::new(url).await?;
    client.set_auth(Some(token)).await;
    let clear =
        BTreeMap::from([("verse".into(), Value::Float64(1_001_001.0)), ("color".into(), Value::Null)]);
    let set = client.mutation("library/highlights:set", clear).await?;
    println!("    highlights.set (no-op) -> {}", show(&set));
    match client.query("library/whoami:get", BTreeMap::new()).await? {
        FunctionResult::Value(v) => Ok(serde_json::Value::from(v)),
        other => bail!("whoami failed: {}", show(&other)),
    }
}

/// Watches `whoami` for `secs`, printing every update with the time since `start`.
async fn watch(client: &mut ConvexClient, start: Instant, secs: u64) -> Result<usize> {
    let mut sub = client.subscribe("library/whoami:get", BTreeMap::new()).await?;
    let mut errors = 0;
    let deadline = start + Duration::from_secs(secs);
    while Instant::now() < deadline {
        match tokio::time::timeout(deadline - Instant::now(), sub.next()).await {
            Ok(Some(r)) => {
                if !matches!(r, FunctionResult::Value(_)) {
                    errors += 1;
                }
                println!("    {:>5.1}s  update: {}", start.elapsed().as_secs_f64(), show(&r));
            }
            Ok(None) => bail!("the subscription ended"),
            Err(_) => break,
        }
    }
    Ok(errors)
}

/// One write and one new query on `client`, printed: whether a token past its `exp` still works.
async fn poke(client: &mut ConvexClient, start: Instant) -> Result<()> {
    let clear =
        BTreeMap::from([("verse".into(), Value::Float64(1_001_001.0)), ("color".into(), Value::Null)]);
    let set = client.mutation("library/highlights:set", clear).await?;
    println!("    {:>5.1}s  mutation: {}", start.elapsed().as_secs_f64(), show(&set));
    let who = client.query("library/whoami:get", BTreeMap::new()).await?;
    println!("    {:>5.1}s  new query: {}", start.elapsed().as_secs_f64(), show(&who));
    Ok(())
}

fn main() -> Result<()> {
    let url = std::env::var("LINGER_CONVEX_URL").context("set LINGER_CONVEX_URL")?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [oauth_cmd, session_cmd] = args.as_slice() else {
        bail!("usage: convex_tokens <oauth cmd> <session cmd>")
    };
    let secs: u64 = std::env::var("SPIKE_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(150);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        println!("1. One tokenIdentifier for both token kinds");
        let a = identity(&url, token(oauth_cmd)?).await?;
        println!("    OAuth access token -> {a}");
        let b = identity(&url, token(session_cmd)?).await?;
        println!("    session token      -> {b}");
        let same = a["tokenIdentifier"] == b["tokenIdentifier"];
        println!("    same tokenIdentifier: {same}");

        println!("2. set_auth_callback with session tokens, watched for {secs} s");
        let calls: Arc<Mutex<Vec<(f64, bool)>>> = Arc::default();
        let start = Instant::now();
        let fetcher: AuthTokenFetcher = {
            let (calls, cmd) = (calls.clone(), session_cmd.clone());
            Box::new(move |force_refresh| {
                let (calls, cmd) = (calls.clone(), cmd.clone());
                Box::pin(async move {
                    calls.lock().unwrap().push((start.elapsed().as_secs_f64(), force_refresh));
                    println!(
                        "    {:>5.1}s  callback(force_refresh={force_refresh})",
                        start.elapsed().as_secs_f64()
                    );
                    Ok(AuthenticationToken::User(token(&cmd)?))
                })
            })
        };
        let mut client = ConvexClient::new(&url).await?;
        client.set_auth_callback(Some(fetcher)).await;
        let errors = watch(&mut client, start, secs).await?;
        poke(&mut client, start).await?;
        tokio::time::sleep(Duration::from_secs(5)).await;
        let calls = calls.lock().unwrap().clone();
        let refreshes = calls.iter().filter(|c| c.1).count();
        println!(
            "    callback calls: {} ({refreshes} with force_refresh), errored updates: {errors}",
            calls.len()
        );
        drop(client);

        println!("3. set_auth with a fresh session token every 30 s, watched for {secs} s");
        let start = Instant::now();
        let mut client = ConvexClient::new(&url).await?;
        client.set_auth(Some(token(session_cmd)?)).await;
        let mut swapper = client.clone();
        let cmd = session_cmd.clone();
        let swaps = tokio::spawn(async move {
            let mut n = 0;
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let Ok(t) = token(&cmd) else { continue };
                swapper.set_auth(Some(t)).await;
                n += 1;
                println!("    {:>5.1}s  set_auth(fresh token) #{n}", start.elapsed().as_secs_f64());
            }
        });
        let errors = watch(&mut client, start, secs).await?;
        swaps.abort();
        println!("    errored updates: {errors}");

        println!("4. Control: set_auth with one session token and no swap, watched for 90 s");
        let start = Instant::now();
        let mut client = ConvexClient::new(&url).await?;
        client.set_auth(Some(token(session_cmd)?)).await;
        let errors = watch(&mut client, start, 90).await?;
        println!("    errored updates: {errors}");
        // With a fixed token the client reconnects with the same expired token forever.
        if tokio::time::timeout(Duration::from_secs(30), poke(&mut client, start)).await.is_err() {
            println!("    {:>5.1}s  no answer to the write within 30 s", start.elapsed().as_secs_f64());
        }
        anyhow::Ok(())
    })
}
