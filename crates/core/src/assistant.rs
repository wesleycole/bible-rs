//! The assistant: Claude, through an Anthropic Messages API endpoint (the Vercel AI Gateway for
//! now, Linger's own proxy later), answering with the [`chat`](crate::chat) tools run here on the
//! device.
//!
//! One question is one [`Conversation::ask`]: the question goes up with what the user is reading,
//! and the reply streams back. When Claude calls a tool the loop runs it against the local
//! translation, sends the result and streams again, until Claude finishes. The conversation keeps
//! every content block exactly as it came back (thinking blocks included) and only ever appends,
//! as the API requires to keep reasoning across turns.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::chat::{Context, Message, Tools};

/// The Gateway's Anthropic-compatible Messages endpoint.
pub const GATEWAY_URL: &str = "https://ai-gateway.vercel.sh/v1/messages";
/// Claude Opus 5.5, by its Gateway name.
pub const MODEL: &str = "anthropic/claude-opus-5.5";
/// Rounds of tool calls one question may take before Linger stops it.
const MAX_ROUNDS: usize = 8;

const SYSTEM: &str = "\
You are the study assistant in Linger, a Bible reading app. The user reads Scripture on the left of \
the screen and talks with you on the right, so your answers sit beside the text.

Ground what you say in the text. Use get_passage to read any verse before you quote or rely on it, \
and search_bible to find where something is said. Quote Scripture only from tool results, in the \
user's translation, never from memory. Cite each verse you draw on as {{Book Chapter:Verse}} or \
{{Book Chapter:Verse-Verse}} with the book's full English name, for example {{John 3:16}} or \
{{1 Corinthians 13:4-7}}; the app turns each citation into a link that opens the passage. Cite only \
verses you have read with a tool.

The user records their church's sermons in Linger, and you can read the transcripts: \
search_sermons finds what was said, read_sermon reads on from a moment. When the user asks what \
their church, pastor or a sermon taught, search the sermons first, and say plainly when nothing was \
found. Cite a moment in a sermon as {{sermon <id> <time>}}, for example {{sermon a40671b1 12:30}}, \
using the id and time a tool gave you. Keep what the preacher said distinct from what Scripture \
says, and quote a preacher only from a transcript. Transcripts are made by machine, so a word that \
looks wrong may be a mishearing.

Beyond the text, answer as a thoughtful, well-read guide would. Where Christian traditions differ, \
say so briefly and fairly rather than taking a side. Keep answers short enough to read in the side \
panel: a few paragraphs at most, separated by blank lines. Write plain prose; the panel shows no \
Markdown, so don't use **bold**, headings, tables or bullet symbols.";

/// Where to send requests, and as whom.
#[derive(Clone)]
pub struct Config {
    pub url: String,
    pub key: String,
    pub model: String,
}

impl Config {
    /// The Gateway, with the key from `LINGER_AI_GATEWAY_KEY` or `AI_GATEWAY_API_KEY`; `None`
    /// when neither is set (the app then falls back to [`chat::local_reply`](crate::chat::local_reply)).
    pub fn from_env() -> Option<Self> {
        let key = ["LINGER_AI_GATEWAY_KEY", "AI_GATEWAY_API_KEY"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))?;
        Some(Self {
            url: std::env::var("LINGER_AI_URL").unwrap_or_else(|_| GATEWAY_URL.into()),
            key,
            model: std::env::var("LINGER_AI_MODEL").unwrap_or_else(|_| MODEL.into()),
        })
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
        if let Some(c) = context {
            content.push(json!({ "type": "text", "text": c.describe(&tools.translation) }));
        }
        content.push(json!({ "type": "text", "text": question }));
        let start = self.messages.len();
        self.messages.push(json!({ "role": "user", "content": content }));

        let agent = agent();
        let mut answer: Vec<String> = Vec::new();
        for _ in 0..MAX_ROUNDS {
            let turn = match stream(&agent, cfg, &self.request(cfg), &mut on_event) {
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

    fn request(&self, cfg: &Config) -> Value {
        json!({
            "model": cfg.model,
            "max_tokens": 16000,
            "stream": true,
            // Thinking is always on for Opus 5.5; effort sets how much. Medium suits questions
            // about a passage; "summarized" makes the wait show as thinking rather than nothing.
            "thinking": { "type": "adaptive", "display": "summarized" },
            "output_config": { "effort": "medium" },
            // A declined request is retried on a model that can answer it.
            "fallbacks": "default",
            // Caches the tools, the system prompt and the conversation so far.
            "cache_control": { "type": "ephemeral" },
            "system": SYSTEM,
            "tools": Tools::definitions(),
            "messages": self.messages,
        })
    }
}

fn agent() -> ureq::Agent {
    // No overall timeout: a long answer streams for as long as it takes. Reads time out if the
    // stream goes quiet.
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(120)))
        .http_status_as_error(false)
        .user_agent(concat!("linger/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// "Reading John 3:16", "Searching for “living water”".
fn tool_label(name: &str, input: &Value) -> String {
    match name {
        "get_passage" => format!("Reading {}", input["reference"].as_str().unwrap_or("a passage")),
        "search_bible" => format!("Searching for “{}”", input["query"].as_str().unwrap_or("")),
        "search_sermons" => format!("Searching sermons for “{}”", input["query"].as_str().unwrap_or("")),
        "read_sermon" => "Reading a sermon".into(),
        _ => "Looking something up".into(),
    }
}

/// Sends one request and reads its event stream, retrying once on overload or a dropped
/// connection before anything has streamed.
fn stream(agent: &ureq::Agent, cfg: &Config, body: &Value, on_event: &mut impl FnMut(Event)) -> Result<Turn> {
    let mut last = None;
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2 * attempt));
        }
        let resp = agent
            .post(&cfg.url)
            .header("content-type", "application/json")
            .header("x-api-key", &cfg.key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "server-side-fallback-2026-07-01")
            .send(body.to_string());
        let mut resp = match resp {
            Ok(r) => r,
            Err(e) => {
                last = Some(anyhow!(e).context("couldn't reach the assistant"));
                continue;
            }
        };
        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            let err = api_error(status, &text);
            if matches!(status, 429 | 500..=599) {
                last = Some(err);
                continue;
            }
            return Err(err);
        }
        let reader = BufReader::new(resp.into_body().into_reader());
        let mut turn = Turn::default();
        let mut data = String::new();
        for line in reader.lines() {
            let line = line.context("the answer stopped arriving")?;
            if let Some(d) = line.strip_prefix("data:") {
                data.push_str(d.trim_start());
            } else if line.is_empty() && !data.is_empty() {
                let event: Value =
                    serde_json::from_str(&data).context("unreadable reply from the assistant")?;
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
        return Ok(turn);
    }
    Err(last.unwrap_or_else(|| anyhow!("couldn't reach the assistant")))
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
