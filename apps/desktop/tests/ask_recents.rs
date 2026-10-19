//! Ask conversations land in Recents: a question and its answer are saved under the question,
//! and a follow-up continues the same saved conversation rather than starting another. Answers
//! come from the offline stand-in (no Gateway key), so nothing goes over the network. Its own
//! test binary because it sets the environment before anything else runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers};
use egui_kittest::Harness;
use linger::app::{BibleApp, Startup, initialize, online_counter};
use linger::theme;
use linger_core::books::{BookId, ChapterIdx};
use linger_core::chat::Role;

fn ask(harness: &mut Harness<'_, BibleApp>, question: &str) {
    harness.event(egui::Event::Text(question.into()));
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    // The stand-in answers on a thread: wait for it.
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
    // SAFETY: set before any harness exists; nothing else reads the environment meanwhile.
    unsafe {
        std::env::set_var("LINGER_OFFSCREEN", "1");
        std::env::remove_var("LINGER_AI_GATEWAY_KEY");
        std::env::remove_var("AI_GATEWAY_API_KEY");
    }
    let dir = std::env::temp_dir().join(format!("linger-ask-recents-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut init = initialize(dir.clone(), Arc::new(|| {})).expect("loading data");
    init.settings.onboarded = true;
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

    let _ = std::fs::remove_dir_all(&dir);
}
