//! Ask conversations land in Recents: a question and its answer are saved under the question,
//! and a follow-up continues the same saved conversation rather than starting another. Answers
//! come from a stand-in for Linger's server on a loopback port, signed in with a static token, so
//! nothing goes over the network. Its own test binary because it sets the environment before
//! anything else runs.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers};
use egui_kittest::Harness;
use linger::app::{BibleApp, Startup, initialize, online_counter};
use linger::theme;
use linger_core::books::{BookId, ChapterIdx};
use linger_core::chat::Role;

/// Linger's server, as Ask uses it: `/v1/me`, and `/v1/ask` streaming a short answer (the third
/// question is refused: the month's questions are used up). Returns its URL and the Ask bodies.
fn mock_server() -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let asks = Arc::new(Mutex::new(Vec::new()));
    let seen = asks.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
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
            let reply = if line.starts_with("GET /v1/me") {
                let me = r#"{"user":{"id":"user_test"},"plan":"free","ask":{"questionsLeft":30,"resetsAt":"2026-11-01T00:00:00.000Z","deepAvailable":false},"sermons":{"hoursLeft":0,"trialAvailable":true}}"#;
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{me}",
                    me.len()
                )
            } else {
                let mut asks = seen.lock().unwrap();
                asks.push(serde_json::from_slice(&body).unwrap());
                if asks.len() == 3 {
                    let e = r#"{"error":{"type":"allowance_exhausted","message":"You've used this month's free questions.","upgrade":true}}"#;
                    format!(
                        "HTTP/1.1 402 Payment Required\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{e}",
                        e.len()
                    )
                } else {
                    let events = [
                        r#"{"type":"message_start","message":{}}"#,
                        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Nicodemus came by night {{John 3:2}}."}}"#,
                        r#"{"type":"content_block_stop","index":0}"#,
                        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
                        r#"{"type":"message_stop"}"#,
                    ];
                    let sse: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nx-linger-questions-left: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
                        30 - asks.len(),
                        sse.len()
                    )
                }
            };
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (url, asks)
}

fn ask(harness: &mut Harness<'_, BibleApp>, question: &str) {
    harness.event(egui::Event::Text(question.into()));
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    // The answer arrives on a thread: wait for it.
    for _ in 0..200 {
        harness.run_steps(1);
        if harness.state().ask.as_ref().is_some_and(|a| !a.waiting()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    harness.run_steps(2);
}

#[test]
fn conversations_are_saved_and_continued() {
    let (server, asks) = mock_server();
    // SAFETY: set before any harness exists; nothing else reads the environment meanwhile.
    unsafe {
        std::env::set_var("LINGER_OFFSCREEN", "1");
        std::env::set_var("LINGER_API_URL", &server);
        std::env::set_var("LINGER_API_TOKEN", "test-token");
        std::env::remove_var("LINGER_AI_GATEWAY_KEY");
        std::env::remove_var("AI_GATEWAY_API_KEY");
    }
    let dir = std::env::temp_dir().join(format!("linger-ask-recents-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut init = initialize(dir.clone(), Arc::new(|| {})).expect("loading data");
    init.settings.onboarded = true;
    init.settings.ask_consent = true;
    init.settings.chapter = ChapterIdx::new(BookId(42), 3).unwrap().0;
    let startup =
        Startup { t0: Instant::now(), probe: false, init: Duration::ZERO, first_ui: None, frames: 0 };
    let mut harness = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| {
        cc.egui_ctx.set_fonts(theme::font_definitions());
        BibleApp::new(cc, init, online_counter(), startup)
    });
    harness.run_steps(4);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::J);
    harness.run_steps(2);
    assert!(harness.state().ask.is_some(), "⌘J opens Ask");

    ask(&mut harness, "Who came to Jesus by night?");
    let chats = harness.state().study.chats();
    assert_eq!(chats.len(), 1, "the answered question is saved");
    assert_eq!(chats[0].title, "Who came to Jesus by night?");
    let id = chats[0].id;
    assert_eq!(harness.state().ask.as_ref().unwrap().chat(), Some(id));

    ask(&mut harness, "What did he ask?");
    let chats = harness.state().study.chats();
    assert_eq!(chats.len(), 1, "a follow-up continues the same conversation");
    let saved = harness.state().study.chat(id).unwrap();
    let roles: Vec<Role> = saved.messages.iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::User, Role::Assistant]);
    assert_eq!(saved.messages[2].text, "What did he ask?");

    // Only the conversation goes up, under the published contract: the server adds the model,
    // the prompt and the tools. The follow-up carries the first exchange, under a new question id.
    {
        let asks = asks.lock().unwrap();
        let (first, second) = (&asks[0], &asks[1]);
        assert_eq!(first["contract"], linger_core::contract::VERSION);
        assert_eq!((first["round"].as_u64(), first["mode"].as_str()), (Some(0), Some("standard")));
        assert!(
            first.get("model").is_none() && first.get("system").is_none() && first.get("tools").is_none()
        );
        assert_eq!(first["messages"].as_array().unwrap().len(), 1);
        assert_eq!(second["messages"].as_array().unwrap().len(), 3);
        assert_ne!(first["questionId"], second["questionId"]);
    }

    // A refused question (the month's questions are used) leaves the saved conversation as it was.
    ask(&mut harness, "And what did Jesus answer?");
    assert_eq!(asks.lock().unwrap().len(), 3);
    assert_eq!(harness.state().study.chat(id).unwrap().messages.len(), 4);

    let _ = std::fs::remove_dir_all(&dir);
}
