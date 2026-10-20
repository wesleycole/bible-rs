//! Linger's own server: the proxy that holds the Claude and AssemblyAI keys, decides what each
//! account may spend, and sells Pro. Every request carries the signed-in user's Clerk token.
//!
//! Errors come back as `{"error": {"type", "message", "upgrade"?, "retryAfter"?}}`; [`ApiError`]
//! sorts them by what the app should do: sign in, show Pro, wait, update, or start a new chat.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

use crate::auth::TokenSource;

/// The production proxy (Vercel's own URL until `api.linger.app` points at it).
pub const API_URL: &str = "https://linger-api.vercel.app";

/// What went wrong talking to the proxy, by what the user can do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// Not signed in, or the session ended (after one refresh).
    SignIn(String),
    /// The allowance is spent (402) or the feature is Pro's (403). `upgrade` says whether Pro
    /// would help (false when already Pro: wait for the reset).
    Upgrade {
        message: String,
        upgrade: bool,
        plan_required: bool,
    },
    /// Rate limited: try again after `retry_after` seconds.
    Wait {
        message: String,
        retry_after: Option<u64>,
    },
    /// This app's Ask contract is too old (409).
    Update(String),
    /// The conversation is too long (413).
    TooLarge(String),
    NotFound(String),
    /// A bug: the server rejected the request (400).
    Invalid(String),
    /// The server or a vendor is down (5xx), or couldn't be reached.
    Unavailable(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = match self {
            ApiError::SignIn(m)
            | ApiError::Upgrade { message: m, .. }
            | ApiError::Wait { message: m, .. }
            | ApiError::Update(m)
            | ApiError::TooLarge(m)
            | ApiError::NotFound(m)
            | ApiError::Invalid(m)
            | ApiError::Unavailable(m) => m,
        };
        f.write_str(m)
    }
}

impl std::error::Error for ApiError {}

impl ApiError {
    /// An error response, by its status and JSON body.
    pub fn from_response(status: u16, body: &str, retry_after_header: Option<&str>) -> Self {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let e = &v["error"];
        let message = e["message"].as_str().map(str::to_string).unwrap_or_else(|| match status {
            401 => "Sign in to continue.".into(),
            429 => "Too many requests; try again in a moment.".into(),
            500..=599 => "Linger's servers are unavailable right now.".into(),
            _ => format!("The server answered HTTP {status}."),
        });
        match (status, e["type"].as_str().unwrap_or("")) {
            (401, _) | (_, "unauthenticated") => ApiError::SignIn(message),
            (402, _) | (403, _) | (_, "allowance_exhausted") | (_, "plan_required") => ApiError::Upgrade {
                upgrade: e["upgrade"].as_bool().unwrap_or(false),
                plan_required: status == 403 || e["type"] == "plan_required",
                message,
            },
            (429, _) | (_, "rate_limited") => ApiError::Wait {
                retry_after: e["retryAfter"]
                    .as_u64()
                    .or_else(|| retry_after_header.and_then(|h| h.trim().parse().ok())),
                message,
            },
            (409, _) => ApiError::Update(message),
            (413, _) => ApiError::TooLarge(message),
            (404, _) => ApiError::NotFound(message),
            (500..=599, _) => ApiError::Unavailable(message),
            _ => ApiError::Invalid(message),
        }
    }
}

/// GET /v1/me.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Me {
    pub user: MeUser,
    /// "free" or "pro".
    pub plan: String,
    #[serde(rename = "renewsAt", default)]
    pub renews_at: Option<String>,
    /// "app_store" or "stripe".
    #[serde(default)]
    pub source: Option<String>,
    pub ask: MeAsk,
    pub sermons: MeSermons,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MeUser {
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(rename = "appAccountToken", default)]
    pub app_account_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MeAsk {
    #[serde(rename = "questionsLeft")]
    pub questions_left: u32,
    #[serde(rename = "resetsAt")]
    pub resets_at: String,
    #[serde(rename = "deepAvailable")]
    pub deep_available: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MeSermons {
    #[serde(rename = "hoursLeft")]
    pub hours_left: f64,
    #[serde(rename = "trialAvailable")]
    pub trial_available: bool,
}

impl Me {
    pub fn is_pro(&self) -> bool {
        self.plan == "pro"
    }
}

/// A client for the proxy, as one signed-in user. Cheap to clone.
#[derive(Clone)]
pub struct Api {
    base: String,
    tokens: Arc<dyn TokenSource>,
    agent: ureq::Agent,
}

/// The production Convex deployment, which the library syncs with ([`convex_url`]).
pub const CONVEX_URL: &str = "https://expert-rabbit-665.convex.cloud";

/// The Convex deployment's URL: `LINGER_CONVEX_URL` (a developer's dev deployment), or production.
pub fn convex_url() -> String {
    std::env::var("LINGER_CONVEX_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| CONVEX_URL.into())
        .trim_end_matches('/')
        .to_string()
}

/// The proxy's URL: `LINGER_API_URL`, or production.
pub fn base_url() -> String {
    std::env::var("LINGER_API_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| API_URL.into())
        .trim_end_matches('/')
        .to_string()
}

pub type Response = ureq::http::Response<ureq::Body>;

impl Api {
    pub fn new(base: impl Into<String>, tokens: Arc<dyn TokenSource>) -> Self {
        // No overall timeout: Ask's answers stream for as long as they take; reads time out if
        // the stream goes quiet.
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_body(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .user_agent(concat!("linger/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { base: base.into().trim_end_matches('/').to_string(), tokens, agent }
    }

    /// The signed-in user's tokens, for the sync engine's own connection.
    pub fn tokens(&self) -> Arc<dyn TokenSource> {
        self.tokens.clone()
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Sends one request with the bearer token; on 401 refreshes the token once and retries.
    /// Returns the response when it is 2xx, else the error it carries.
    pub fn send(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Response, ApiError> {
        let url = format!("{}{path}", self.base);
        let token = self.tokens.token().map_err(|e| ApiError::SignIn(format!("{e:#}")))?;
        let mut resp = self.request(method, &url, &token, body)?;
        if resp.status().as_u16() == 401 {
            match self.tokens.refresh() {
                Ok(fresh) => resp = self.request(method, &url, &fresh, body)?,
                Err(_) => return Err(error_of(resp)),
            }
        }
        if resp.status().is_success() { Ok(resp) } else { Err(error_of(resp)) }
    }

    fn request(
        &self,
        method: &str,
        url: &str,
        token: &str,
        body: Option<&Value>,
    ) -> Result<Response, ApiError> {
        let auth = format!("Bearer {token}");
        let unreachable =
            |e: ureq::Error| ApiError::Unavailable(format!("Couldn't reach Linger's servers: {e}"));
        match (method, body) {
            ("GET", _) => self.agent.get(url).header("authorization", &auth).call().map_err(unreachable),
            ("DELETE", _) => {
                self.agent.delete(url).header("authorization", &auth).call().map_err(unreachable)
            }
            (_, body) => self
                .agent
                .post(url)
                .header("authorization", &auth)
                .header("content-type", "application/json")
                .send(body.map_or_else(|| "{}".to_string(), Value::to_string))
                .map_err(unreachable),
        }
    }

    fn json(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, ApiError> {
        let mut resp = self.send(method, path, body)?;
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|_| ApiError::Unavailable("Unreadable reply from Linger's servers.".into()))
    }

    pub fn me(&self) -> Result<Me, ApiError> {
        let v = self.json("GET", "/v1/me", None)?;
        serde_json::from_value(v)
            .map_err(|e| ApiError::Unavailable(format!("Unreadable account details: {e}")))
    }

    /// A Stripe Checkout page for Pro, to open in the browser.
    pub fn checkout(&self, yearly: bool) -> Result<String, ApiError> {
        let body = serde_json::json!({ "interval": if yearly { "yearly" } else { "monthly" } });
        url_of(self.json("POST", "/v1/billing/stripe/checkout", Some(&body))?)
    }

    /// The Stripe Customer Portal, to manage or cancel the subscription.
    pub fn portal(&self) -> Result<String, ApiError> {
        url_of(self.json("POST", "/v1/billing/stripe/portal", None)?)
    }

    /// Deletes the account: the Clerk user, Linger's records of it, and any audio in flight.
    pub fn delete_account(&self) -> Result<(), ApiError> {
        self.send("DELETE", "/v1/account", None).map(drop)
    }

    pub(crate) fn json_call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, ApiError> {
        self.json(method, path, body)
    }
}

fn url_of(v: Value) -> Result<String, ApiError> {
    v["url"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| ApiError::Unavailable("The server sent no link.".into()))
}

/// The error a non-2xx response carries.
pub fn error_of(mut resp: Response) -> ApiError {
    let status = resp.status().as_u16();
    let retry = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = resp.body_mut().read_to_string().unwrap_or_default();
    ApiError::from_response(status, &body, retry.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_sort_by_what_the_user_can_do() {
        let e = |status, body: &str| ApiError::from_response(status, body, None);
        assert_eq!(
            e(402, r#"{"error":{"type":"allowance_exhausted","message":"Used up.","upgrade":true}}"#),
            ApiError::Upgrade { message: "Used up.".into(), upgrade: true, plan_required: false }
        );
        assert_eq!(
            e(403, r#"{"error":{"type":"plan_required","message":"Pro.","upgrade":true}}"#),
            ApiError::Upgrade { message: "Pro.".into(), upgrade: true, plan_required: true }
        );
        assert_eq!(
            e(429, r#"{"error":{"type":"rate_limited","message":"Slow.","retryAfter":42}}"#),
            ApiError::Wait { message: "Slow.".into(), retry_after: Some(42) }
        );
        // The header stands in when the body has no retryAfter.
        assert_eq!(
            ApiError::from_response(429, "", Some("7")),
            ApiError::Wait {
                message: "Too many requests; try again in a moment.".into(),
                retry_after: Some(7)
            }
        );
        assert!(matches!(e(401, "{}"), ApiError::SignIn(_)));
        assert!(matches!(
            e(409, r#"{"error":{"type":"contract_unsupported","message":"Update"}}"#),
            ApiError::Update(_)
        ));
        assert!(matches!(
            e(413, r#"{"error":{"type":"too_large","message":"Long"}}"#),
            ApiError::TooLarge(_)
        ));
        assert!(matches!(e(503, "<html>"), ApiError::Unavailable(_)));
        assert!(matches!(
            e(400, r#"{"error":{"type":"invalid_request","message":"Bad"}}"#),
            ApiError::Invalid(_)
        ));
    }

    #[test]
    fn me_parses_with_optional_fields_missing() {
        let me: Me = serde_json::from_str(
            r#"{"user":{"id":"user_1","appAccountToken":"u"},"plan":"free",
                "ask":{"questionsLeft":29,"resetsAt":"2026-11-01T00:00:00.000Z","deepAvailable":false},
                "sermons":{"hoursLeft":0,"trialAvailable":true}}"#,
        )
        .unwrap();
        assert!(!me.is_pro());
        assert_eq!(me.ask.questions_left, 29);
        assert_eq!(me.user.email, None);
        assert!(me.sermons.trial_available);
    }
}
