//! Renders the app offscreen to a PNG, for design comparisons and screenshots, without opening a
//! window (so it works whatever is on screen, and on a machine without one):
//!
//! ```text
//! LINGER_HOME=<data dir> LINGER_DEMO="palette:jn 3:16" \
//!     cargo run --release --example shot -- out.png [1280x800]
//! ```
//!
//! `LINGER_SHOT_CLICKS` and `LINGER_SHOT_HOVER` (points, `x,y`) click and point after the
//! demo steps, for states only the pointer reaches, such as an open menu.
//! `LINGER_SHOT_PAUSE_MS` waits that long after each frame, so chapters of an online translation
//! can arrive before the picture is taken.
//!
//! `LINGER_SHOT_FILMSTRIP=<n>` then presses ⌘B and writes the next n frames at 120 Hz as
//! `<out>-01.png` …, to see the sidebar's slide frame by frame. Other renders show transitions
//! already finished.
//!
//! egui's test harness drives the real app with wgpu at 2× scale. On macOS the window's traffic
//! lights belong to AppKit, so they are drawn onto the image where AppKit puts them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use linger::app::{BibleApp, Startup, bump, initialize, online_counter};
use linger::theme;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(out) = args.first() else {
        eprintln!("usage: shot <out.png> [WIDTHxHEIGHT]");
        std::process::exit(2);
    };
    let (w, h) = args
        .get(1)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?)))
        .unwrap_or((1280.0, 800.0));
    if std::env::var_os("LINGER_OFFSCREEN").is_none() {
        // Offscreen is what lays the app out as in a window, and what keeps a render from
        // starting a real sync engine (against production, without LINGER_CONVEX_URL).
        // Keep this ahead of any thread and of initialize(): the app reads offscreen() to decide
        // whether to start sync, so anything built above this line would start the real engine.
        // SAFETY: no other thread has started yet.
        unsafe { std::env::set_var("LINGER_OFFSCREEN", "1") };
    }
    // Online text that arrives bumps the counter, so the reader lays its chapters out again.
    let online = online_counter();
    let notify = {
        let online = online.clone();
        Arc::new(move || bump(&online)) as Arc<dyn Fn() + Send + Sync>
    };
    let init = initialize(linger::data_dir(), notify).expect("loading data");
    let startup =
        Startup { t0: Instant::now(), probe: false, init: Duration::ZERO, first_ui: None, frames: 0 };
    let filmstrip: Option<usize> = std::env::var("LINGER_SHOT_FILMSTRIP").ok().and_then(|s| s.parse().ok());
    let mut builder = Harness::builder().with_size(egui::vec2(w, h)).with_pixels_per_point(2.0);
    if filmstrip.is_some() {
        builder = builder.with_step_dt(1.0 / 120.0);
    }
    let mut harness = builder.wgpu().build_eframe(|cc| {
        cc.egui_ctx.set_fonts(theme::font_definitions());
        BibleApp::new(cc, init, online, startup)
    });
    // The harness sets its own (dark) egui theme after the app has applied the reading theme,
    // which would leave egui's widgets (menus, fields) in stock dark style.
    let ctx = harness.ctx.clone();
    theme::apply(&ctx, harness.state().settings.theme);
    ctx.all_styles_mut(|style| style.animation_time = 0.0);
    // A font atlas as large as the GPU's (egui assumes 2048 px without one): a small atlas fills
    // at 2× and egui rebuilds it, leaving the reader's cached text pointing at the old glyphs.
    harness.input_mut().max_texture_side = Some(8192);
    harness.step();
    harness.state_mut().reader.invalidate();
    let demo_steps = std::env::var("LINGER_DEMO").map(|s| s.split(';').count()).unwrap_or(0);
    let steps =
        std::env::var("LINGER_SHOT_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(30 + 4 * demo_steps);
    let pause =
        std::env::var("LINGER_SHOT_PAUSE_MS").ok().and_then(|s| s.parse().ok()).map(Duration::from_millis);
    for _ in 0..steps {
        harness.step();
        if let Some(pause) = pause {
            std::thread::sleep(pause);
        }
    }
    // `LINGER_SHOT_CLICKS="x,y;x,y"`: click those points in turn (opening a menu from its
    // button, say), then `LINGER_SHOT_HOVER="x,y"`: leave the pointer there.
    let point = |s: &str| {
        let (x, y) = s.split_once(',')?;
        Some(egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))
    };
    let clicks = std::env::var("LINGER_SHOT_CLICKS").unwrap_or_default();
    for pos in clicks.split(';').filter_map(point) {
        harness.hover_at(pos);
        harness.step();
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
        }
        harness.run_steps(8);
    }
    if let Some(pos) = std::env::var("LINGER_SHOT_HOVER").ok().as_deref().and_then(point) {
        harness.hover_at(pos);
        harness.run_steps(8);
    }
    // `LINGER_SHOT_KEYS="ArrowRight;Enter"`: then press those keys (egui's names) in turn, for
    // states the keyboard reaches.
    let keys = std::env::var("LINGER_SHOT_KEYS").unwrap_or_default();
    for name in keys.split(';').filter(|s| !s.is_empty()) {
        let Some(key) = egui::Key::from_name(name.trim()) else {
            eprintln!("LINGER_SHOT_KEYS: unknown key {name:?}");
            continue;
        };
        for pressed in [true, false] {
            harness.event(egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
        }
        harness.run_steps(4);
    }
    let save = |harness: &mut Harness<'_, BibleApp>, path: &str| {
        let mut img = harness.render().expect("rendering");
        if cfg!(target_os = "macos") && std::env::var_os("LINGER_NO_TRAFFIC_LIGHTS").is_none() {
            traffic_lights(&mut img);
        }
        img.save(path).expect("writing the image");
    };
    let Some(frames) = filmstrip else {
        save(&mut harness, out);
        return;
    };
    // The frame that handles ⌘B (the harness runs one frame per event, so the key's release
    // comes with the slide's first frame), then the slide.
    let key = |pressed| egui::Event::Key {
        key: egui::Key::B,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    };
    harness.event(key(true));
    harness.step();
    harness.event(key(false));
    let stem = out.strip_suffix(".png").unwrap_or(out);
    for i in 1..=frames {
        harness.step();
        save(&mut harness, &format!("{stem}-{i:02}.png"));
    }
}

/// AppKit's traffic lights for a 52 pt unified toolbar: 14 pt circles 9 pt apart, the first
/// centred 27 pt from the left, on the 26 pt centre line, each with a faint inner hairline.
fn traffic_lights(img: &mut image::RgbaImage) {
    let s = 2.0;
    for (i, c) in [theme::traffic_lights::CLOSE, theme::traffic_lights::MINIMIZE, theme::traffic_lights::ZOOM]
        .into_iter()
        .enumerate()
    {
        let (cx, cy, r) = ((27.0 + 23.0 * i as f32) * s, 26.0 * s, 7.0 * s);
        let [cr, cg, cb, _] = c.to_srgba_unmultiplied();
        for y in (cy - r - 2.0) as u32..=(cy + r + 2.0) as u32 {
            for x in (cx - r - 2.0) as u32..=(cx + r + 2.0) as u32 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let cover = (r - d + 0.5).clamp(0.0, 1.0);
                if cover <= 0.0 || x >= img.width() || y >= img.height() {
                    continue;
                }
                // The hairline: a ring 0.5 pt wide inside the edge, 12 % black.
                let ring = (1.0 - ((r - 0.5) - d).abs()).clamp(0.0, 1.0) * 0.12;
                let px = img.get_pixel_mut(x, y);
                for (k, v) in [cr, cg, cb].into_iter().enumerate() {
                    let fill = v as f32 * (1.0 - ring);
                    px[k] = (px[k] as f32 * (1.0 - cover) + fill * cover).round() as u8;
                }
            }
        }
    }
}
