//! The reader holds one chapter: scrolling stops at its end, and → turns the page. With
//! continuous scrolling on, the same keys scroll on into the next chapters. Its own test binary
//! because it sets `LINGER_OFFSCREEN` before anything else runs (see `book_picker_app.rs`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key};
use egui_kittest::Harness;
use linger::app::{BibleApp, Startup, initialize, online_counter};
use linger::theme;
use linger_core::books::{BookId, ChapterIdx};

const JOHN: BookId = BookId(42);

fn reader(continuous: bool) -> (Harness<'static, BibleApp>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("linger-chapter-{continuous}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut init = initialize(dir.clone(), Arc::new(|| {})).expect("loading data");
    init.settings.onboarded = true;
    init.settings.chapter = ChapterIdx::new(JOHN, 3).unwrap().0;
    init.settings.continuous_scroll = continuous;
    let startup =
        Startup { t0: Instant::now(), probe: false, init: Duration::ZERO, first_ui: None, frames: 0 };
    let mut harness = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| {
        cc.egui_ctx.set_fonts(theme::font_definitions());
        BibleApp::new(cc, init, online_counter(), startup)
    });
    harness.run_steps(4);
    (harness, dir)
}

fn page_down(harness: &mut Harness<'_, BibleApp>, times: usize) {
    for _ in 0..times {
        harness.key_press(Key::PageDown);
        harness.run_steps(30);
    }
}

#[test]
fn scrolling_stays_in_the_chapter_unless_continuous() {
    // SAFETY: set before any harness exists; nothing else reads the environment meanwhile.
    unsafe { std::env::set_var("LINGER_OFFSCREEN", "1") };
    let john = |n| ChapterIdx::new(JOHN, n).unwrap();

    let (mut harness, dir) = reader(false);
    page_down(&mut harness, 12);
    assert_eq!(harness.state().reader.anchor.chapter, john(3), "scrolling stops at the chapter's end");
    assert_eq!(harness.state().reader.location.0, john(3));
    harness.key_press(Key::ArrowRight);
    harness.run_steps(4);
    assert_eq!(harness.state().reader.location.0, john(4), "→ turns to the next chapter");
    assert_eq!(harness.state().reader.anchor.offset, 0.0, "at its start");
    let _ = std::fs::remove_dir_all(&dir);

    let (mut harness, dir) = reader(true);
    page_down(&mut harness, 12);
    assert!(harness.state().reader.anchor.chapter > john(3), "continuous scrolling runs on");
    let _ = std::fs::remove_dir_all(&dir);
}
