//! Signing in to Linger's account, for Ask and cloud transcripts.
//!
//! The desktop app uses Clerk as an OAuth 2.0 provider: a public client with PKCE (S256), the
//! system browser, and a loopback redirect (`http://127.0.0.1:<port>/callback`, RFC 8252). The
//! refresh token and the current access token, with its expiry, are kept together as one item in
//! the OS credential store ([`SecretStore`]), so a process that starts inside the access token's
//! life makes no token request at all. A grant happens only shortly before the token expires, or
//! once after the proxy answers 401, and its answer is stored before the new token is used: Clerk
//! rotates the refresh token on every grant, so a process killed between a grant and that write
//! would leave a dead one behind.
//!
//! Requests ask a [`TokenSource`] for the bearer token, so the same client code works with a
//! signed-in [`OAuthSession`] or, in development, a pasted [`StaticToken`].

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::secrets::SecretStore;

/// Clerk's Frontend API for the Linger application: its development instance until a production
/// instance has a domain (`clerk.trylinger.com`).
pub const CLERK_URL: &str = "https://elegant-mink-67.clerk.accounts.dev";
/// The "Linger Desktop" OAuth application's client id (public: PKCE, no secret).
pub const CLIENT_ID: &str = "XDg0tXKW3CgO9rJy";
pub const SCOPES: &str = "openid profile email offline_access";
/// Where the session is kept: a JSON [`Saved`], or (written before access tokens were kept) the
/// bare refresh token.
pub const REFRESH_KEY: &str = "clerk-refresh-token";
/// A token pasted in from Settings in development builds.
pub const DEV_TOKEN_KEY: &str = "dev-clerk-token";
/// Refresh this long before the access token expires.
const EARLY: Duration = Duration::from_secs(60);

/// A bearer token for Linger's proxy.
pub trait TokenSource: Send + Sync {
    /// A token that should be valid now, refreshed first if it's about to expire.
    fn token(&self) -> Result<String>;
    /// A fresh token after the proxy rejected the current one; an error means sign in again.
    fn refresh(&self) -> Result<String>;
}

/// One fixed token: the dev Settings field, or `LINGER_API_TOKEN`. It can't be refreshed.
pub struct StaticToken(pub String);

impl TokenSource for StaticToken {
    fn token(&self) -> Result<String> {
        Ok(self.0.clone())
    }

    fn refresh(&self) -> Result<String> {
        bail!("the token was rejected")
    }
}

/// Where the OAuth endpoints are, and as which client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthConfig {
    /// Clerk's Frontend API URL, without a trailing slash.
    pub issuer: String,
    pub client_id: String,
}

impl OAuthConfig {
    /// The built-in Clerk application, overridden by `LINGER_CLERK_URL` and `LINGER_OAUTH_CLIENT_ID`.
    pub fn from_env() -> Self {
        let get =
            |k: &str, d: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty()).unwrap_or(d.into());
        Self {
            issuer: get("LINGER_CLERK_URL", CLERK_URL).trim_end_matches('/').to_string(),
            client_id: get("LINGER_OAUTH_CLIENT_ID", CLIENT_ID),
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.issuer)
    }
}

/// The secrets of one sign-in attempt: the PKCE verifier and the anti-forgery `state`.
#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    pub fn new() -> Self {
        let verifier = base64url(&random_bytes(32));
        Self { challenge: challenge(&verifier), state: base64url(&random_bytes(16)), verifier }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

/// The S256 code challenge for a verifier (RFC 7636 §4.2).
pub fn challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

/// Random bytes from the OS (through `uuid`'s v4 generator, already a dependency).
fn random_bytes(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n + 16);
    while out.len() < n {
        out.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    out.truncate(n);
    out
}

/// Base64url without padding.
pub fn base64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut s = String::with_capacity(bytes.len() * 4 / 3 + 3);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..=chunk.len() {
            s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    s
}

/// When a JWT expires (its `exp`, Unix seconds), read without checking the signature: for
/// scheduling a refresh, not for trusting the token.
pub fn expires_at(jwt: &str) -> Option<u64> {
    let payload = jwt.split('.').nth(1)?;
    let mut bytes = Vec::with_capacity(payload.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in payload.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((acc >> bits) as u8);
        }
    }
    serde_json::from_slice::<serde_json::Value>(&bytes).ok()?["exp"].as_u64()
}

/// Percent-encodes a query or form value (RFC 3986 unreserved characters pass through).
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A query string's value for `key`.
fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (decode(k) == key).then(|| decode(v))
    })
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&")
}

/// The authorization URL the browser opens.
pub fn authorize_url(cfg: &OAuthConfig, pkce: &Pkce, redirect_uri: &str) -> String {
    format!(
        "{}?{}",
        cfg.endpoint("/oauth/authorize"),
        form(&[
            ("response_type", "code"),
            ("client_id", &cfg.client_id),
            ("redirect_uri", redirect_uri),
            ("scope", SCOPES),
            ("state", &pkce.state),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
        ])
    )
}

/// What Clerk's token endpoint returns.
#[derive(Debug, Clone, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Seconds.
    #[serde(default)]
    pub expires_in: Option<u64>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .user_agent(concat!("linger/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn post_form(url: &str, body: String) -> Result<(u16, String)> {
    let mut resp = agent()
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .send(body)
        .map_err(|e| anyhow!(e).context("couldn't reach the sign-in service"))?;
    let status = resp.status().as_u16();
    Ok((status, resp.body_mut().read_to_string().unwrap_or_default()))
}

/// Clerk refused the grant itself (`invalid_grant`): the refresh token is dead, and nothing but
/// signing in again brings the session back.
#[derive(Debug)]
pub struct InvalidGrant(pub String);

impl std::fmt::Display for InvalidGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sign-in failed: {}", self.0)
    }
}

impl std::error::Error for InvalidGrant {}

fn token_request(cfg: &OAuthConfig, pairs: &[(&str, &str)]) -> Result<Tokens> {
    let (status, text) = post_form(&cfg.endpoint("/oauth/token"), form(pairs))?;
    if status != 200 {
        let body = serde_json::from_str::<serde_json::Value>(&text).ok();
        let why = body
            .as_ref()
            .and_then(|v| v["error_description"].as_str().or(v["error"].as_str()).map(str::to_string))
            .unwrap_or_else(|| format!("HTTP {status}"));
        if status == 400 && body.as_ref().is_some_and(|v| v["error"] == "invalid_grant") {
            return Err(InvalidGrant(why).into());
        }
        bail!("sign-in failed: {why}");
    }
    serde_json::from_str(&text).context("unreadable reply from the sign-in service")
}

/// Trades the authorization code for tokens (step 3 of the flow).
pub fn exchange_code(cfg: &OAuthConfig, code: &str, verifier: &str, redirect_uri: &str) -> Result<Tokens> {
    token_request(
        cfg,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", &cfg.client_id),
            ("code_verifier", verifier),
        ],
    )
}

pub fn refresh_tokens(cfg: &OAuthConfig, refresh_token: &str) -> Result<Tokens> {
    token_request(
        cfg,
        &[("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", &cfg.client_id)],
    )
}

/// Revokes a refresh token (RFC 7009); Clerk's never expire on their own.
pub fn revoke(cfg: &OAuthConfig, token: &str) -> Result<()> {
    let body = form(&[("token", token), ("token_type_hint", "refresh_token"), ("client_id", &cfg.client_id)]);
    let (status, _) = post_form(&cfg.endpoint("/oauth/token/revoke"), body)?;
    if status != 200 {
        bail!("couldn't revoke the sign-in (HTTP {status})");
    }
    Ok(())
}

/// A listener on a free loopback port, waiting for the browser to come back with the code.
pub struct Loopback {
    listener: TcpListener,
    pub redirect_uri: String,
}

impl Loopback {
    pub fn bind() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").context("couldn't listen for the sign-in")?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, redirect_uri: format!("http://127.0.0.1:{port}/callback") })
    }

    /// Waits for `/callback`, checks `state`, and returns the code. Requests for anything else
    /// (a browser asking for a favicon) are answered 404 and the wait goes on.
    pub fn wait(&self, state: &str, timeout: Duration) -> Result<String> {
        self.listener.set_nonblocking(true)?;
        let deadline = Instant::now() + timeout;
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false)?;
                    if let Some(result) = handle_callback(stream, state)? {
                        return result;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() > deadline {
                        bail!("sign-in timed out");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

const PAGE: &str = include_str!("auth_callback.html");
const LOGO: &str = include_str!("../../../apps/desktop/assets/icon.svg");

/// `None` for a request that isn't the callback.
fn handle_callback(mut stream: TcpStream, state: &str) -> Result<Option<Result<String>>> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let target = line.split_whitespace().nth(1).unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/callback" {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        return Ok(None);
    }
    let result = if let Some(err) = query_value(query, "error") {
        let why = query_value(query, "error_description").unwrap_or(err);
        Err(anyhow!("sign-in was cancelled: {why}"))
    } else if query_value(query, "state").as_deref() != Some(state) {
        Err(anyhow!("the sign-in reply didn't match this attempt"))
    } else {
        query_value(query, "code").context("the sign-in reply had no code")
    };
    // The app still has to exchange the code and save the tokens after this response.
    let (title, message) = match &result {
        Ok(_) => ("Welcome to Linger.", "Return to the app to finish signing in."),
        Err(_) => ("We couldn't finish signing in.", "Return to Linger and try signing in again."),
    };
    let body = PAGE.replace("{{title}}", title).replace("{{message}}", message).replace("{{logo}}", LOGO);
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncache-control: no-store\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    Ok(Some(result))
}

/// How this device is signed in, if it is: `LINGER_API_TOKEN` (scripts and tests), a token pasted
/// into the development Settings field (debug builds only), or the last browser sign-in.
pub fn saved(store: &SecretStore) -> Option<Arc<dyn TokenSource>> {
    if let Some(t) = std::env::var("LINGER_API_TOKEN").ok().filter(|v| !v.trim().is_empty()) {
        return Some(Arc::new(StaticToken(t.trim().to_string())));
    }
    if cfg!(debug_assertions)
        && let Some(t) = store.get(DEV_TOKEN_KEY)
    {
        return Some(Arc::new(StaticToken(t)));
    }
    OAuthSession::restore(OAuthConfig::from_env(), store.clone()).map(|s| Arc::new(s) as Arc<dyn TokenSource>)
}

/// Signs this device out: revokes and forgets the browser sign-in, and forgets a pasted
/// development token. The local sign-out happens even when revoking fails; that error is returned
/// after it.
pub fn sign_out(store: &SecretStore) -> Result<()> {
    let revoked = match OAuthSession::restore(OAuthConfig::from_env(), store.clone()) {
        Some(s) => s.sign_out(),
        None => Ok(()),
    };
    store.delete(DEV_TOKEN_KEY)?;
    revoked
}

/// The session as stored: both tokens in one item, so a write can't leave one without the other.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Saved {
    refresh: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access: Option<String>,
    /// When `access` expires, Unix seconds.
    #[serde(default)]
    exp: u64,
}

impl Saved {
    fn load(store: &SecretStore) -> Option<Self> {
        let raw = store.get(REFRESH_KEY)?;
        Some(serde_json::from_str(&raw).unwrap_or(Saved { refresh: raw, access: None, exp: 0 }))
    }

    /// The access token, if it has longer than [`EARLY`] left, with when to stop using it.
    fn access(&self) -> Option<(String, Instant)> {
        let left = Duration::from_secs(self.exp).checked_sub(unix_now())?.checked_sub(EARLY)?;
        Some((self.access.clone()?, Instant::now() + left))
    }
}

fn unix_now() -> Duration {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()
}

/// A signed-in account: refreshes its access token as needed.
pub struct OAuthSession {
    cfg: OAuthConfig,
    store: SecretStore,
    /// The access token and when to stop using it.
    access: Mutex<Option<(String, Instant)>>,
}

impl OAuthSession {
    /// The browser sign-in: opens Clerk's hosted sign-in with `open`, waits for the redirect,
    /// trades the code and keeps the refresh token.
    pub fn sign_in(cfg: OAuthConfig, store: SecretStore, open: impl FnOnce(&str)) -> Result<Self> {
        // TODO(macOS): ASWebAuthenticationSession (objc2-authentication-services) with the
        // https://auth.trylinger.com/oauth/callback Associated Domains redirect. The loopback flow
        // below works on macOS too, so it is used there until that lands.
        let loopback = Loopback::bind()?;
        let pkce = Pkce::new();
        open(&authorize_url(&cfg, &pkce, &loopback.redirect_uri));
        let code = loopback.wait(&pkce.state, Duration::from_secs(5 * 60))?;
        let tokens = exchange_code(&cfg, &code, &pkce.verifier, &loopback.redirect_uri)?;
        let session = Self { cfg, store, access: Mutex::new(None) };
        session.keep(tokens)?;
        Ok(session)
    }

    /// The session left by the last sign-in, if a refresh token is stored.
    pub fn restore(cfg: OAuthConfig, store: SecretStore) -> Option<Self> {
        store.get(REFRESH_KEY)?;
        Some(Self { cfg, store, access: Mutex::new(None) })
    }

    /// Stores a grant's answer, refresh and access token in one write, before the token is used.
    fn keep(&self, tokens: Tokens) -> Result<String> {
        self.keep_in(&mut self.access.lock().unwrap_or_else(|e| e.into_inner()), tokens)
    }

    fn keep_in(&self, access: &mut Option<(String, Instant)>, tokens: Tokens) -> Result<String> {
        let refresh = match tokens.refresh_token {
            Some(r) => r,
            None => Saved::load(&self.store).context("sign in to Linger")?.refresh,
        };
        let life = tokens
            .expires_in
            .or_else(|| expires_at(&tokens.access_token).map(|exp| exp.saturating_sub(unix_now().as_secs())));
        let exp = unix_now().as_secs() + life.unwrap_or(3600);
        let saved = Saved { refresh, access: Some(tokens.access_token.clone()), exp };
        self.store.set(REFRESH_KEY, &serde_json::to_string(&saved)?)?;
        *access = saved.access();
        Ok(tokens.access_token)
    }

    /// Revokes the refresh token and forgets it. The local sign-out happens even if Clerk can't
    /// be reached.
    pub fn sign_out(&self) -> Result<()> {
        let revoked = match Saved::load(&self.store) {
            Some(s) => revoke(&self.cfg, &s.refresh),
            None => Ok(()),
        };
        self.store.delete(REFRESH_KEY)?;
        *self.access.lock().unwrap_or_else(|e| e.into_inner()) = None;
        revoked
    }
}

impl TokenSource for OAuthSession {
    fn token(&self) -> Result<String> {
        let mut access = self.access.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, until)) = &*access
            && Instant::now() < *until
        {
            return Ok(t.clone());
        }
        // A token stored by an earlier run, or by another process since, is used as it is.
        if let Some((t, until)) = Saved::load(&self.store).and_then(|s| s.access()) {
            *access = Some((t.clone(), until));
            return Ok(t);
        }
        self.grant(&mut access, None)
    }

    fn refresh(&self) -> Result<String> {
        let mut access = self.access.lock().unwrap_or_else(|e| e.into_inner());
        // The token the proxy rejected: the one in use, or failing that the stored one.
        let rejected = access
            .as_ref()
            .map(|(t, _)| t.clone())
            .or_else(|| Saved::load(&self.store).and_then(|s| s.access));
        self.grant(&mut access, rejected)
    }
}

impl OAuthSession {
    /// Trades the stored refresh token for new tokens, unless another process already has: under
    /// a lock across processes, it reads the store again and uses a stored token that is still
    /// good and isn't `rejected`. Clerk revokes a refresh token's whole family when one is used
    /// twice, so two processes must never send the same one. The caller holds the access lock,
    /// so the threads sharing this session (the UI and sync) don't run two grants either.
    fn grant(&self, access: &mut Option<(String, Instant)>, rejected: Option<String>) -> Result<String> {
        let _lock = self.store.lock().context("couldn't lock the sign-in")?;
        let saved = Saved::load(&self.store).context("sign in to Linger")?;
        if let Some((t, until)) = saved.access()
            && Some(&t) != rejected.as_ref()
        {
            *access = Some((t.clone(), until));
            return Ok(t);
        }
        match refresh_tokens(&self.cfg, &saved.refresh) {
            Ok(tokens) => self.keep_in(access, tokens),
            Err(e) if e.is::<InvalidGrant>() => {
                // The session is dead for good: forget it, so every caller sees signed out (the
                // sign-in prompt) rather than this error on every call.
                self.store.delete(REFRESH_KEY)?;
                *access = None;
                Err(e.context("not signed in: the sign-in ended; sign in again"))
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expires_at_reads_a_jwts_exp() {
        let payload = base64url(br#"{"sub":"user_1","exp":1792460000}"#);
        assert_eq!(expires_at(&format!("eyJhbGciOiJSUzI1NiJ9.{payload}.sig")), Some(1_792_460_000));
        assert_eq!(expires_at("not a jwt"), None);
    }

    #[test]
    fn base64url_matches_rfc4648_vectors() {
        // RFC 4648 §10, unpadded, with the URL alphabet.
        for (input, out) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64url(input.as_bytes()), out);
        }
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn s256_challenge_matches_rfc7636_appendix_b() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let p = Pkce::new();
        assert_eq!(p.verifier.len(), 43, "32 random bytes");
        assert_ne!(p.verifier, Pkce::new().verifier);
    }

    #[test]
    fn query_values_are_decoded() {
        assert_eq!(query_value("code=a%2Fb+c&state=x", "code").as_deref(), Some("a/b c"));
        assert_eq!(query_value("code=1", "state"), None);
        assert_eq!(decode("100%"), "100%");
        assert_eq!(encode("a b/c~"), "a%20b%2Fc~");
    }

    #[test]
    fn authorize_url_carries_pkce_and_state() {
        let cfg = OAuthConfig { issuer: "https://clerk.example".into(), client_id: "cid".into() };
        let p = Pkce { verifier: "v".into(), challenge: "ch".into(), state: "st".into() };
        let url = authorize_url(&cfg, &p, "http://127.0.0.1:5000/callback");
        let (base, q) = url.split_once('?').unwrap();
        assert_eq!(base, "https://clerk.example/oauth/authorize");
        assert_eq!(query_value(q, "code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(query_value(q, "code_challenge").as_deref(), Some("ch"));
        assert_eq!(query_value(q, "state").as_deref(), Some("st"));
        assert_eq!(query_value(q, "redirect_uri").as_deref(), Some("http://127.0.0.1:5000/callback"));
        assert_eq!(query_value(q, "scope").as_deref(), Some(SCOPES));
        assert!(!q.contains("code_verifier"), "the verifier never goes to the browser");
    }

    /// Requests a mock server saw: (path, body).
    #[cfg(not(target_os = "macos"))]
    type Seen = Arc<Mutex<Vec<(String, String)>>>;

    /// A stand-in for Clerk's token endpoints: answers each POST from `reply(path, form)` and
    /// keeps every request it saw.
    #[cfg(not(target_os = "macos"))]
    fn mock_clerk(reply: impl Fn(&str, &str) -> (u16, String) + Send + 'static) -> (OAuthConfig, Seen) {
        use std::io::Read;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut length = 0;
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    if h.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':')
                        && k.eq_ignore_ascii_case("content-length")
                    {
                        length = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8(body).unwrap();
                let (status, json) = reply(&path, &body);
                log.lock().unwrap().push((path, body));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{json}",
                    json.len()
                );
            }
        });
        (OAuthConfig { issuer, client_id: "cid".into() }, seen)
    }

    fn get(url: &str) -> (u16, String) {
        let mut resp = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .new_agent()
            .get(url)
            .call()
            .unwrap();
        (resp.status().as_u16(), resp.body_mut().read_to_string().unwrap())
    }

    // The session keeps its refresh token in the SecretStore, which is the Keychain on macOS.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn browser_sign_in_refresh_and_sign_out_against_a_mock_clerk() {
        let (cfg, seen) = mock_clerk(|path, form| match (path, query_value(form, "grant_type").as_deref()) {
            ("/oauth/token", Some("authorization_code")) => {
                (200, r#"{"access_token":"at-1","refresh_token":"rt-1","expires_in":3600}"#.into())
            }
            ("/oauth/token", Some("refresh_token")) if form.contains("refresh_token=rt-1") => {
                (200, r#"{"access_token":"at-2","refresh_token":"rt-2","expires_in":3600}"#.into())
            }
            ("/oauth/token", Some("refresh_token")) if form.contains("refresh_token=rt-2") => {
                (200, r#"{"access_token":"at-3","refresh_token":"rt-3","expires_in":3600}"#.into())
            }
            ("/oauth/token/revoke", _) => (200, "{}".into()),
            _ => (400, r#"{"error":"invalid_grant","error_description":"no such grant"}"#.into()),
        });
        let dir = std::env::temp_dir().join(format!("linger-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SecretStore::new(&dir);

        // The "browser": a stray favicon request first (answered 404, the wait goes on), then the
        // redirect back with the code and the attempt's state.
        let session = OAuthSession::sign_in(cfg.clone(), store.clone(), |url| {
            let q = url.split_once('?').unwrap().1;
            let redirect = query_value(q, "redirect_uri").unwrap();
            let state = query_value(q, "state").unwrap();
            assert_eq!(query_value(q, "code_challenge").map(|c| c.len()), Some(43));
            std::thread::spawn(move || {
                let base = redirect.trim_end_matches("/callback");
                assert_eq!(get(&format!("{base}/favicon.ico")).0, 404);
                let (status, page) = get(&format!("{redirect}?code=the-code&state={state}"));
                assert_eq!(status, 200);
                assert!(page.contains("Return to the app to finish signing in."), "{page}");
            });
        })
        .unwrap();
        assert_eq!(session.token().unwrap(), "at-1");
        assert_eq!(Saved::load(&store).unwrap().refresh, "rt-1");
        {
            let seen = seen.lock().unwrap();
            let (path, form) = &seen[0];
            assert_eq!(path, "/oauth/token");
            assert_eq!(query_value(form, "code").as_deref(), Some("the-code"));
            assert_eq!(query_value(form, "client_id").as_deref(), Some("cid"));
            // The verifier goes only to the token endpoint, and matches the challenge sent.
            assert_eq!(query_value(form, "code_verifier").map(|v| v.len()), Some(43));
        }

        // A 401 from the proxy refreshes; the rotated refresh token replaces the old one, and a
        // restored session (next launch) uses it.
        assert_eq!(session.refresh().unwrap(), "at-2");
        assert_eq!(Saved::load(&store).unwrap().refresh, "rt-2");
        let restored = OAuthSession::restore(cfg.clone(), store.clone()).unwrap();
        assert_eq!(restored.refresh().unwrap(), "at-3");

        session.sign_out().unwrap();
        assert_eq!(store.get(REFRESH_KEY), None);
        assert!(OAuthSession::restore(cfg, store).is_none());
        let seen = seen.lock().unwrap();
        let (path, form) = seen.last().unwrap();
        assert_eq!(path, "/oauth/token/revoke");
        assert_eq!(query_value(form, "token").as_deref(), Some("rt-3"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Each start of the app or the CLI is a new session restored from the store.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_start_within_the_tokens_life_makes_no_grant_and_one_after_it_makes_one() {
        let (cfg, seen) = mock_clerk(|_, form| match query_value(form, "refresh_token").as_deref() {
            Some("rt-1") => {
                (200, r#"{"access_token":"at-2","refresh_token":"rt-2","expires_in":3600}"#.into())
            }
            _ => (400, r#"{"error":"invalid_grant"}"#.into()),
        });
        let dir = std::env::temp_dir().join(format!("linger-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SecretStore::new(&dir);
        let save = |access: &str, exp: u64| {
            let saved = Saved { refresh: "rt-1".into(), access: Some(access.into()), exp };
            store.set(REFRESH_KEY, &serde_json::to_string(&saved).unwrap()).unwrap();
        };
        let start = || OAuthSession::restore(cfg.clone(), store.clone()).unwrap();
        let now = unix_now().as_secs();

        save("at-1", now + 3600);
        assert_eq!(start().token().unwrap(), "at-1");
        assert_eq!(start().token().unwrap(), "at-1");
        assert!(seen.lock().unwrap().is_empty(), "no grant inside the token's life");

        // Inside EARLY of expiry: one grant, stored, and the next start uses what it stored.
        save("at-1", now + 30);
        assert_eq!(start().token().unwrap(), "at-2");
        assert_eq!(start().token().unwrap(), "at-2");
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(Saved::load(&store).unwrap().refresh, "rt-2");

        // A store written before access tokens were kept holds the bare refresh token.
        store.set(REFRESH_KEY, "rt-1").unwrap();
        assert_eq!(start().token().unwrap(), "at-2");
        assert_eq!(seen.lock().unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Two processes (here two sessions, as two starts are) on one store.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn two_starts_past_expiry_make_one_grant_between_them_and_a_dead_token_signs_out() {
        let (cfg, seen) = mock_clerk(|_, form| {
            // Slow enough that both starts are inside the grant at once without the lock.
            std::thread::sleep(Duration::from_millis(200));
            match query_value(form, "refresh_token").as_deref() {
                Some("rt-1") => {
                    (200, r#"{"access_token":"at-2","refresh_token":"rt-2","expires_in":3600}"#.into())
                }
                _ => (400, r#"{"error":"invalid_grant","error_description":"reused"}"#.into()),
            }
        });
        let dir = std::env::temp_dir().join(format!("linger-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = SecretStore::new(&dir);
        let saved =
            Saved { refresh: "rt-1".into(), access: Some("at-1".into()), exp: unix_now().as_secs() + 30 };
        store.set(REFRESH_KEY, &serde_json::to_string(&saved).unwrap()).unwrap();
        let starts: Vec<_> = (0..2)
            .map(|_| {
                let session = OAuthSession::restore(cfg.clone(), store.clone()).unwrap();
                std::thread::spawn(move || session.token().unwrap())
            })
            .collect();
        let tokens: Vec<String> = starts.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(tokens, ["at-2", "at-2"]);
        assert_eq!(seen.lock().unwrap().len(), 1, "one grant between them");

        // A refresh token Clerk refuses is forgotten: the next start is signed out.
        let session = OAuthSession::restore(cfg.clone(), store.clone()).unwrap();
        assert!(session.refresh().unwrap_err().chain().any(|e| e.is::<InvalidGrant>()));
        assert!(OAuthSession::restore(cfg, store).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn loopback_page_guides_the_user_back_to_the_app() {
        for (query, error) in [
            ("code=private-code&state=mine", None),
            ("code=private-code&state=someone-else", Some("didn't match")),
            ("state=mine", Some("no code")),
            (
                "error=access_denied&error_description=%3Cscript%3Eprivate-error%3C%2Fscript%3E&state=mine",
                Some("cancelled"),
            ),
        ] {
            let loopback = Loopback::bind().unwrap();
            let url = format!("{}?{query}", loopback.redirect_uri);
            let browser = std::thread::spawn(move || get(&url));
            let result = loopback.wait("mine", Duration::from_secs(10));
            let (status, page) = browser.join().unwrap();
            assert_eq!(status, 200);
            if let Some(error) = error {
                assert!(result.unwrap_err().to_string().contains(error));
                assert!(page.contains("<h1>We couldn't finish signing in.</h1>"));
                assert!(page.contains("Return to Linger and try signing in again."));
                assert!(!page.contains("Welcome to Linger."));
            } else {
                assert_eq!(result.unwrap(), "private-code");
                assert!(page.contains("<h1>Welcome to Linger.</h1>"));
                assert!(page.contains("Return to the app to finish signing in."));
            }
            assert!(page.contains(LOGO), "the logo must be inline; the listener closes now");
            assert!(page.contains("You can close this tab."));
            assert!(!page.contains("private-code"));
            assert!(!page.contains("private-error"));
            assert!(!page.contains("{{"), "all template slots must be filled");
        }
    }
}
