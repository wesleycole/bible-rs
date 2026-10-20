//! The assistant: Claude, through Linger's proxy (or, for development, the Vercel AI Gateway
//! directly), answering with the [`chat`](crate::chat) tools run here on the device.
//!
//! One question is one [`Conversation::ask`]: the question goes up with what the user is reading,
//! and the reply streams back. When Claude calls a tool the loop runs it against the local
//! translation, sends the result and streams again, until Claude finishes. The conversation keeps
//! every content block exactly as it came back (thinking blocks included) and only ever appends,
//! as the API requires to keep reasoning across turns.
//!
//! Through the proxy the app sends only the conversation, with a question id and the round; the
//! proxy adds the model, the system prompt and the tools from the published
//! [`contract`](crate::contract), and decides what the account may spend.

use std::io::{BufRead, BufReader};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::api::{Api, Http, Request, Response};
use crate::chat::{self, Context, Message, Tools};
use crate::contract;

/// The Gateway's Anthropic-compatible Messages endpoint.
pub const GATEWAY_URL: &str = "https://ai-gateway.vercel.sh/v1/messages";
/// The model direct mode asks for, unless `LINGER_AI_MODEL` names another.
pub const MODEL: &str = contract::direct::MODEL;

/// Standard questions, or Deep study (a stronger model; Pro, counts as two questions).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Standard,
    Deep,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Standard => "standard",
            Mode::Deep => "deep",
        }
    }
}

/// Where requests go.
#[derive(Clone)]
pub enum Backend {
    /// Linger's proxy, as the signed-in user.
    Proxy(Api),
    /// Development only: the Gateway itself, with a key from the environment.
    Direct { url: String, key: String, model: String, http: Arc<dyn Http> },
}

/// Where to send requests, as whom, and in which mode.
#[derive(Clone)]
pub struct Config {
    pub backend: Backend,
    pub mode: Mode,
}

impl Config {
    /// Through Linger's proxy.
    pub fn proxy(api: Api, mode: Mode) -> Self {
        Self { backend: Backend::Proxy(api), mode }
    }

    /// Development mode: the Gateway, with the key from `LINGER_AI_GATEWAY_KEY` or
    /// `AI_GATEWAY_API_KEY`; `None` when neither is set.
    #[cfg(feature = "native")]
    pub fn from_env() -> Option<Self> {
        let key = ["LINGER_AI_GATEWAY_KEY", "AI_GATEWAY_API_KEY"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))?;
        Some(Self {
            backend: Backend::Direct {
                url: std::env::var("LINGER_AI_URL").unwrap_or_else(|_| GATEWAY_URL.into()),
                key,
                model: std::env::var("LINGER_AI_MODEL").unwrap_or_else(|_| MODEL.into()),
                http: Arc::new(crate::api::Ureq::default()),
            },
            mode: Mode::Standard,
        })
    }

    pub fn is_proxy(&self) -> bool {
        matches!(self.backend, Backend::Proxy(_))
    }
}

/// What the UI hears while an answer is coming.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// More of the answer's text.
    Text(String),
    /// Claude is reading or searching: "Reading John 3:16".
    Tool(String),
    /// Claude is thinking (between tool calls, or before it writes).
    Thinking,
    /// The proxy's word on the account, with each round: questions left this month, and
    /// whether this question was moved to a lighter model past the month's cost ceiling.
    Allowance { questions_left: Option<u32>, downgraded: bool },
}

/// One conversation with Claude, as the API sees it.
#[derive(Default)]
pub struct Conversation {
    /// `messages`, append-only: every assistant turn exactly as returned.
    messages: Vec<Value>,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    /// The history as JSON, to save and carry on from later ([`Conversation::from_json`]).
    pub fn to_json(&self) -> String {
        Value::Array(self.messages.clone()).to_string()
    }

    /// A saved history; anything unreadable starts afresh.
    pub fn from_json(s: &str) -> Self {
        let messages = match serde_json::from_str(s) {
            Ok(Value::Array(m)) => m,
            _ => Vec::new(),
        };
        Self { messages }
    }

    /// Asks `question` and runs the tool loop until Claude is done, reporting progress to
    /// `on_event`. Returns the answer: all of its text, citations included.
    pub fn ask(
        &mut self,
        cfg: &Config,
        tools: &Tools,
        context: Option<&Context>,
        question: &str,
        mut on_event: impl FnMut(Event),
    ) -> Result<Message> {
        let mut content = Vec::new();
        // What the user is reading, which translation the tools read and whether they have
        // sermons: per question, so the system prompt stays the same for everyone.
        let note = chat::tools_note(tools);
        let about = match context {
            Some(c) => format!("{} {note}", c.describe()),
            None => note,
        };
        content.push(json!({ "type": "text", "text": about }));
        content.push(json!({ "type": "text", "text": question }));
        let start = self.messages.len();
        self.messages.push(json!({ "role": "user", "content": content }));

        let mut answer: Vec<String> = Vec::new();
        // One id for the question and all its tool rounds: the proxy counts the question once
        // and keeps every round on the model it started on.
        let question_id = uuid::Uuid::new_v4().to_string();
        for round in 0..contract::MAX_ROUNDS {
            let body = self.request(cfg, &question_id, round);
            let turn = match stream(cfg, &body, &mut on_event) {
                Ok(t) => t,
                Err(e) => {
                    // Nothing of this question stays, so the next one starts from a valid history.
                    self.messages.truncate(start);
                    return Err(e);
                }
            };
            answer.extend(turn.text());
            self.messages.push(json!({ "role": "assistant", "content": turn.blocks }));
            match turn.stop_reason.as_deref() {
                Some("tool_use") => {
                    let results: Vec<Value> = turn
                        .blocks
                        .iter()
                        .filter(|b| b["type"] == "tool_use")
                        .map(|b| {
                            let (name, input) = (b["name"].as_str().unwrap_or(""), &b["input"]);
                            on_event(Event::Tool(tool_label(name, input)));
                            let out = tools.run(name, input);
                            let (text, error) = match out {
                                Ok(v) => (v.to_string(), false),
                                Err(e) => (format!("{e:#}"), true),
                            };
                            json!({ "type": "tool_result", "tool_use_id": b["id"], "content": text, "is_error": error })
                        })
                        .collect();
                    // Every result of a round in one message, so Claude keeps calling in parallel.
                    self.messages.push(json!({ "role": "user", "content": results }));
                }
                Some("refusal") => {
                    answer.push("Linger's assistant can't help with that one.".into());
                    break;
                }
                Some("max_tokens") => {
                    answer.push("(The answer was cut off here.)".into());
                    break;
                }
                _ => break,
            }
        }
        if answer.is_empty() {
            bail!("the assistant stopped without answering");
        }
        Ok(Message::assistant(answer.join("\n\n")))
    }

    fn request(&self, cfg: &Config, question_id: &str, round: usize) -> Value {
        match &cfg.backend {
            // The proxy injects the model, system prompt, tools and settings itself.
            Backend::Proxy(_) => json!({
                "contract": contract::VERSION,
                "questionId": question_id,
                "round": round,
                "mode": cfg.mode.as_str(),
                "messages": self.messages,
            }),
            Backend::Direct { model, .. } => json!({
                "model": model,
                "max_tokens": contract::direct::MAX_TOKENS,
                "stream": true,
                // Thinking is always on for Opus 5.5; effort sets how much. Medium suits
                // questions about a passage; "summarized" makes the wait show as thinking.
                "thinking": { "type": "adaptive", "display": "summarized" },
                "output_config": { "effort": contract::direct::EFFORT },
                // A declined request is retried on a model that can answer it.
                "fallbacks": "default",
                // Caches the tools, the system prompt and the conversation so far.
                "cache_control": { "type": "ephemeral" },
                "system": contract::SYSTEM,
                "tools": Tools::definitions(),
                "messages": self.messages,
            }),
        }
    }
}

/// "Reading John 3:16", "Searching for “living water”".
fn tool_label(name: &str, input: &Value) -> String {
    match name {
        "get_passage" => format!("Reading {}", input["reference"].as_str().unwrap_or("a passage")),
        "search_bible" => format!("Searching for “{}”", input["query"].as_str().unwrap_or("")),
        "search_sermons" => format!("Searching sermons for “{}”", input["query"].as_str().unwrap_or("")),
        "read_sermon" => "Reading a sermon".into(),
        "original_words" => {
            let r = input["reference"].as_str().unwrap_or("");
            let language = match crate::reference::parse(r).map(|r| r.book.testament()) {
                Some(crate::Testament::New) => "Greek",
                Some(crate::Testament::Old) => "Hebrew",
                None => "original",
            };
            format!("Reading the {language} of {r}")
        }
        "lexicon" => format!("Looking up {}", chat::word_label(input["word"].as_str().unwrap_or(""))),
        "word_occurrences" => {
            format!("Finding where {} is used", chat::word_label(input["word"].as_str().unwrap_or("")))
        }
        "cross_references" => {
            format!("Finding cross-references for {}", input["reference"].as_str().unwrap_or("a passage"))
        }
        "people_and_places" => match (input["name"].as_str(), input["reference"].as_str()) {
            (Some(name), _) => format!("Looking up {}", chat::name_label(name)),
            (None, Some(r)) => format!("Finding who is named in {r}"),
            (None, None) => "Looking up a name".into(),
        },
        _ => "Looking something up".into(),
    }
}

/// Sends one request and reads its event stream.
///
/// Through the proxy a refused request is the proxy's decision (sign in, allowance, rate) and
/// comes back as an [`ApiError`] for the panel to show; the client only refreshes its token
/// once on a 401. Direct to the Gateway it retries twice on overload or a dropped connection
/// before anything has streamed.
fn stream(cfg: &Config, body: &Value, on_event: &mut impl FnMut(Event)) -> Result<Turn> {
    let (url, key, http) = match &cfg.backend {
        Backend::Proxy(api) => {
            let resp = api.send("POST", "/v1/ask", Some(body))?;
            on_event(Event::Allowance {
                questions_left: resp.header("x-linger-questions-left").and_then(|v| v.parse().ok()),
                downgraded: resp.header("x-linger-downgraded") == Some("1"),
            });
            return read_events(resp, on_event);
        }
        Backend::Direct { url, key, http, .. } => (url, key, http),
    };
    let mut last = None;
    for attempt in 0..3 {
        if attempt > 0 {
            // The web has no thread to sleep; it retries at once.
            #[cfg(not(target_arch = "wasm32"))]
            std::thread::sleep(std::time::Duration::from_secs(2 * attempt));
        }
        let req = Request::new("POST", url)
            .header("content-type", "application/json")
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "server-side-fallback-2026-07-01")
            .body(body.to_string());
        let mut resp = match http.send(req) {
            Ok(r) => r,
            Err(e) => {
                last = Some(anyhow!(e).context("couldn't reach the assistant"));
                continue;
            }
        };
        if resp.status != 200 {
            let text = resp.text();
            let err = api_error(resp.status, &text);
            if matches!(resp.status, 429 | 500..=599) {
                last = Some(err);
                continue;
            }
            return Err(err);
        }
        return read_events(resp, on_event);
    }
    Err(last.unwrap_or_else(|| anyhow!("couldn't reach the assistant")))
}

/// Folds a Messages event stream into one turn, reporting progress as it goes.
fn read_events(resp: Response, on_event: &mut impl FnMut(Event)) -> Result<Turn> {
    let reader = BufReader::new(resp.body);
    let mut turn = Turn::default();
    let mut data = String::new();
    for line in reader.lines() {
        let line = line.context("the answer stopped arriving")?;
        if let Some(d) = line.strip_prefix("data:") {
            data.push_str(d.trim_start());
        } else if line.is_empty() && !data.is_empty() {
            let event: Value = serde_json::from_str(&data).context("unreadable reply from the assistant")?;
            data.clear();
            if let Some(e) = turn.apply(&event)? {
                on_event(e);
            }
            if turn.done {
                break;
            }
        }
    }
    if !turn.done {
        bail!("the answer stopped arriving");
    }
    Ok(turn)
}

fn api_error(status: u16, body: &str) -> anyhow::Error {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.chars().take(200).collect());
    match status {
        401 | 403 => anyhow!("the assistant's key was rejected (HTTP {status})"),
        429 => anyhow!("the assistant is busy; try again in a moment (HTTP 429)"),
        _ => anyhow!("the assistant returned an error (HTTP {status}): {message}"),
    }
}

/// One assistant turn, rebuilt from its stream.
#[derive(Default, Debug)]
struct Turn {
    blocks: Vec<Value>,
    /// Tool input arrives as JSON text in pieces; parsed when its block ends.
    partial_json: Vec<String>,
    stop_reason: Option<String>,
    done: bool,
}

impl Turn {
    fn text(&self) -> impl Iterator<Item = String> + '_ {
        self.blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }

    /// Folds in one stream event.
    fn apply(&mut self, e: &Value) -> Result<Option<Event>> {
        let index = e["index"].as_u64().unwrap_or(0) as usize;
        Ok(match e["type"].as_str().unwrap_or("") {
            "content_block_start" => {
                let block = e["content_block"].clone();
                let thinking = block["type"] == "thinking" || block["type"] == "redacted_thinking";
                self.blocks.resize(index + 1, Value::Null);
                self.partial_json.resize(index + 1, String::new());
                self.blocks[index] = block;
                thinking.then_some(Event::Thinking)
            }
            "content_block_delta" => {
                let d = &e["delta"];
                let block = self.blocks.get_mut(index).context("a reply block arrived out of order")?;
                let append = |block: &mut Value, field: &str, more: &str| {
                    let s = format!("{}{more}", block[field].as_str().unwrap_or(""));
                    block[field] = Value::String(s);
                };
                match d["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        let t = d["text"].as_str().unwrap_or("");
                        append(block, "text", t);
                        Some(Event::Text(t.to_string()))
                    }
                    "thinking_delta" => {
                        append(block, "thinking", d["thinking"].as_str().unwrap_or(""));
                        None
                    }
                    "signature_delta" => {
                        append(block, "signature", d["signature"].as_str().unwrap_or(""));
                        None
                    }
                    "input_json_delta" => {
                        self.partial_json[index].push_str(d["partial_json"].as_str().unwrap_or(""));
                        None
                    }
                    _ => None,
                }
            }
            "content_block_stop" => {
                if let Some(block) = self.blocks.get_mut(index)
                    && block["type"] == "tool_use"
                {
                    let raw = std::mem::take(&mut self.partial_json[index]);
                    if !raw.trim().is_empty() {
                        block["input"] = serde_json::from_str(&raw).context("unreadable tool call")?;
                    }
                }
                None
            }
            "message_delta" => {
                if let Some(r) = e["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(r.to_string());
                }
                None
            }
            "message_stop" => {
                self.done = true;
                None
            }
            "error" => bail!(
                "the assistant returned an error: {}",
                e["error"]["message"].as_str().unwrap_or("unknown")
            ),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(events: &[Value]) -> (Turn, Vec<Event>) {
        let mut t = Turn::default();
        let out = events.iter().filter_map(|e| t.apply(e).unwrap()).collect();
        (t, out)
    }

    #[test]
    fn rebuilds_thinking_text_and_tool_calls_from_the_stream() {
        let (t, events) = fold(&[
            json!({"type":"message_start","message":{}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Look it "}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"up."}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Let me "}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"read it."}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"get_passage","input":{}}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"reference\": \"John 3"}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":":16\"}"}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
            json!({"type":"message_stop"}),
        ]);
        assert!(t.done);
        assert_eq!(t.stop_reason.as_deref(), Some("tool_use"));
        // The thinking block goes back exactly as it was built, signature and all.
        assert_eq!(t.blocks[0], json!({"type":"thinking","thinking":"Look it up.","signature":"sig"}));
        assert_eq!(t.blocks[2]["input"], json!({"reference": "John 3:16"}));
        assert_eq!(t.text().collect::<Vec<_>>(), ["Let me read it."]);
        assert_eq!(events, [Event::Thinking, Event::Text("Let me ".into()), Event::Text("read it.".into())]);
    }

    #[test]
    fn a_tool_call_without_arguments_keeps_an_empty_input() {
        let (t, _) = fold(&[
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"x","input":{}}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_stop"}),
        ]);
        assert_eq!(t.blocks[0]["input"], json!({}));
    }

    #[test]
    fn stream_errors_surface() {
        let mut t = Turn::default();
        let e = t.apply(&json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}));
        assert!(e.unwrap_err().to_string().contains("Overloaded"));
    }
}
