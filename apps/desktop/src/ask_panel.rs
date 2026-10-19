//! Ask: a conversation about what is being read, docked right of the reader (concept A).
//!
//! The reader feeds the conversation (the chapter and any selected verses go with each question,
//! shown as a chip that can be dropped) and the conversation points back at the reader: every
//! verse an answer cites is a chip that jumps there, and gets a dot in the reader's margin.
//! Answers come from Claude through `linger_core::assistant`, streamed in as they're written;
//! without a Gateway key, from `linger_core::chat`'s labelled stand-in.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use eframe::egui::text::LayoutJob;
use eframe::egui::{
    self, Align2, FontId, Key, Modifiers, Rect, ScrollArea, Sense, TextFormat, Ui, pos2, vec2,
};
use linger_core::assistant::{self, Conversation, Event};
use linger_core::chat::{self, Message, Role, Segment};
use linger_core::reference::{self, Reference};
use linger_core::transcribe::{self, Transcript};
use linger_core::{ChapterIdx, VerseRef};

use crate::actions::Action;
use crate::icons;
use crate::theme::{self, Palette, text};
use crate::widgets;

/// A 372 pt card 8 pt from the window's edge: a little wider than the compare inspector, for
/// prose.
pub const ASK_WIDTH: f32 = 380.0;
/// Rounder than the sidebar's 16 pt: the card and its composer read as soft, separate objects.
pub const CARD_RADIUS: f32 = 20.0;
const COMPOSER_RADIUS: f32 = 20.0;
const PAD: f32 = 16.0;
const HEADER: f32 = 52.0;
const COMPOSER_INSET: f32 = 12.0;

/// A white card on the page: the panel's hairline ring, with a wider, softer drop shadow than the
/// sidebar's so it lifts off a page of the same colour.
pub fn paint_card(painter: &egui::Painter, p: &Palette, r: Rect) {
    let lift = card_lift(p);
    painter.add(lift.drop_shape(r, CARD_RADIUS));
    painter.rect_filled(r, CARD_RADIUS, p.surface);
    lift.paint_ring(painter, r, CARD_RADIUS);
}

/// The card's shadow and ring.
pub fn card_lift(p: &Palette) -> theme::Elevation {
    theme::Elevation {
        y: 4.0,
        blur: 20.0,
        color: p.shadows.popover.color.gamma_multiply(0.85),
        ..p.shadows.panel
    }
}

#[derive(Default)]
pub struct AskPanel {
    /// What the panel shows; the API's view of the same conversation is `conversation`.
    messages: Vec<Message>,
    conversation: Arc<Mutex<Conversation>>,
    /// The transcribed sermons, as of the last question: the tools search them, and citations
    /// of them are labelled from them.
    sermons: Arc<Vec<Transcript>>,
    config: Option<assistant::Config>,
    input: String,
    pending: Option<Receiver<Update>>,
    /// The answer so far while it streams, and what Claude is doing ("Reading John 3:16").
    streaming: String,
    status: Option<String>,
    /// The reading context was dropped from the next question.
    context_off: bool,
    focus: bool,
    stick_to_bottom: bool,
    /// The saved conversation this continues (Recents), its title, and whether an answer has
    /// arrived since it was last saved.
    chat: Option<i64>,
    title: Option<String>,
    unsaved: bool,
}

/// What the panel needs from the app each frame.
pub struct AskEnv<'a> {
    pub library: &'a linger_core::library::Library,
    pub translation: &'a str,
    pub chapter: ChapterIdx,
    pub selection: Option<(VerseRef, VerseRef)>,
    pub data_dir: &'a std::path::Path,
}

enum Update {
    Event(Event),
    Done(Message),
}

impl AskPanel {
    pub fn new() -> Self {
        Self { focus: true, config: assistant::Config::from_env(), ..Default::default() }
    }

    /// A saved conversation, to carry on from where it was left.
    pub fn open(saved: linger_core::study::SavedChat) -> Self {
        Self {
            messages: saved.messages,
            conversation: Arc::new(Mutex::new(Conversation::from_json(&saved.api))),
            chat: Some(saved.id),
            title: Some(saved.title),
            stick_to_bottom: true,
            ..Self::new()
        }
    }

    /// The saved conversation shown, if it has been saved.
    pub fn chat(&self) -> Option<i64> {
        self.chat
    }

    /// Saves the conversation if an answer has arrived since it was last saved: as a new chat in
    /// Recents, titled with its first question, or over the one it continues.
    pub fn save(&mut self, study: &mut linger_core::study::StudyStore) -> anyhow::Result<()> {
        if !std::mem::take(&mut self.unsaved) {
            return Ok(());
        }
        let first = self.messages.iter().find(|m| m.role == Role::User).map_or("", |m| m.text.as_str());
        let title = self.title.get_or_insert_with(|| title_for(first)).clone();
        let api = self.conversation.lock().unwrap_or_else(|e| e.into_inner()).to_json();
        self.chat = Some(study.save_chat(self.chat, &title, &self.messages, &api)?);
        Ok(())
    }

    /// Focus the question field next frame.
    pub fn focus(&mut self) {
        self.focus = true;
        self.context_off = false;
    }

    /// The verses the latest answer cites, for the reader's margin.
    pub fn cited(&self) -> Vec<(VerseRef, VerseRef)> {
        self.messages.iter().rev().find(|m| m.role == Role::Assistant).map(Message::cited).unwrap_or_default()
    }

    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// Draws the panel's content in `ui` (its card already painted). Returns `true` to close.
    pub fn show(&mut self, ui: &mut Ui, p: &Palette, env: &AskEnv<'_>, actions: &mut Vec<Action>) -> bool {
        while let Some(update) = self.pending.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match update {
                Update::Event(Event::Text(t)) => {
                    self.streaming.push_str(&t);
                    self.status = None;
                }
                Update::Event(Event::Tool(what)) => self.status = Some(what),
                Update::Event(Event::Thinking) => self.status = Some("Thinking".into()),
                Update::Done(m) => {
                    self.messages.push(m);
                    self.unsaved = true;
                    self.pending = None;
                    self.streaming.clear();
                    self.status = None;
                }
            }
            self.stick_to_bottom = true;
        }
        if self.pending.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
        }
        let rect = ui.max_rect();
        let close = self.header(ui, p, rect);
        let context =
            (!self.context_off).then_some(chat::Context { chapter: env.chapter, selection: env.selection });

        // The composer floats at the foot of the card and grows with its text, up to five lines:
        // the question on top, the context chip and send button on a row beneath it.
        let composer_w = rect.width() - 2.0 * COMPOSER_INSET;
        let lines = wrapped_lines(ui, &self.input, composer_w - 32.0).clamp(1, 5) as f32;
        let composer_h = 14.0 + 20.0 * lines + 8.0 + 32.0 + 8.0;
        let composer = Rect::from_min_size(
            pos2(rect.left() + COMPOSER_INSET, rect.bottom() - COMPOSER_INSET - composer_h),
            vec2(composer_w, composer_h),
        );

        let body = Rect::from_min_max(
            pos2(rect.left(), rect.top() + HEADER),
            pos2(rect.right(), composer.top() - 8.0),
        );
        let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
        body_ui.set_clip_rect(body);
        self.conversation(&mut body_ui, p, env, actions);

        if self.composer(ui, p, env, composer) {
            self.send(env, context);
        }
        close
    }

    fn header(&mut self, ui: &mut Ui, p: &Palette, rect: Rect) -> bool {
        let cy = rect.top() + HEADER / 2.0;
        icons::sparkle(
            ui.painter(),
            Rect::from_center_size(pos2(rect.left() + PAD + 10.0, cy), vec2(20.0, 20.0)),
            p.accent,
        );
        ui.painter().text(
            pos2(rect.left() + PAD + 28.0, cy),
            Align2::LEFT_CENTER,
            "Ask",
            text::UI_TITLE.font(),
            p.ink,
        );
        let mut close = false;
        let close_rect = Rect::from_center_size(pos2(rect.right() - 12.0 - 14.0, cy), vec2(28.0, 28.0));
        if widgets::round_button(ui, p, close_rect, icons::close, egui::Id::new("ask-close"))
            .on_hover_text("Close (⌘J)")
            .clicked()
        {
            close = true;
        }
        if !self.messages.is_empty() {
            let new_rect = close_rect.translate(vec2(-32.0, 0.0));
            if widgets::round_button(ui, p, new_rect, icons::plus, egui::Id::new("ask-new"))
                .on_hover_text("New conversation")
                .clicked()
                && self.pending.is_none()
            {
                self.messages.clear();
                self.conversation = Arc::default();
                self.chat = None;
                self.title = None;
                self.focus = true;
            }
        }
        close
    }

    fn conversation(&mut self, ui: &mut Ui, p: &Palette, env: &AskEnv<'_>, actions: &mut Vec<Action>) {
        let width = ui.max_rect().width() - 2.0 * PAD;
        if self.messages.is_empty() && self.pending.is_none() {
            let r = ui.max_rect();
            let msg = "Ask about what you're reading. Answers cite the verses they draw on, and \
                       later the sermons you've recorded.";
            let g = layout(ui, msg, FontId::new(13.0, theme::family(theme::UI)), p.ink_2, width, 20.0);
            let h = g.size().y;
            ui.painter().galley(pos2(r.left() + PAD, r.center().y - h / 2.0), g, p.ink_2);
            return;
        }
        let stick = std::mem::take(&mut self.stick_to_bottom);
        ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            ui.add_space(4.0);
            for m in &self.messages {
                match m.role {
                    Role::User => user_bubble(ui, p, &m.text, width),
                    Role::Assistant => answer(ui, p, env, &self.sermons, &m.text, width, actions),
                }
                ui.add_space(14.0);
            }
            if self.pending.is_some() {
                // The answer so far, without a citation still being written.
                let shown = match self.streaming.rfind("{{") {
                    Some(i) if !self.streaming[i..].contains("}}") => &self.streaming[..i],
                    _ => &self.streaming,
                };
                if !shown.trim().is_empty() {
                    answer(ui, p, env, &self.sermons, shown, width, actions);
                    ui.add_space(8.0);
                }
                if self.status.is_some() || shown.trim().is_empty() {
                    let dots = ".".repeat(1 + (ui.input(|i| i.time) * 3.0) as usize % 3);
                    let what = self.status.as_deref().unwrap_or("Thinking");
                    let (r, _) = ui.allocate_exact_size(vec2(width + 2.0 * PAD, 24.0), Sense::hover());
                    ui.painter().text(
                        pos2(r.left() + PAD, r.center().y),
                        Align2::LEFT_CENTER,
                        format!("{what}{dots}"),
                        text::UI.font(),
                        p.ink_3,
                    );
                }
            }
            ui.add_space(8.0);
            if stick {
                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
            }
        });
    }

    /// "Reading John 3 · verse 14 selected", with an × that leaves the next question without it.
    fn context_chip(&mut self, ui: &mut Ui, p: &Palette, env: &AskEnv<'_>, at: Rect) {
        let label =
            if self.context_off { format!("Add {}", env.chapter.display()) } else { context_label(env) };
        let font = text::SMALL.font();
        let g = ui.painter().layout_no_wrap(label, font, if self.context_off { p.ink_2 } else { p.accent });
        let trailing = if self.context_off { 10.0 } else { 26.0 };
        let r = Rect::from_min_size(at.min, vec2(10.0 + g.size().x + trailing, at.height()));
        ui.painter().rect_filled(r, r.height() / 2.0, if self.context_off { p.fill } else { p.accent_soft });
        ui.painter().galley(pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, p.accent);
        let resp = ui.interact(r, egui::Id::new("ask-context"), Sense::click());
        if self.context_off {
            if resp.on_hover_text("Send what you're reading with the question").clicked() {
                self.context_off = false;
            }
        } else {
            let x = Rect::from_center_size(pos2(r.right() - 13.0, r.center().y), vec2(10.0, 10.0));
            icons::close(ui.painter(), x, p.accent);
            if resp.on_hover_text("Don't send this with the question").clicked() {
                self.context_off = true;
            }
        }
    }

    /// The composer card: the question field, then the context chip and the send button; `true`
    /// when a question is sent.
    fn composer(&mut self, ui: &mut Ui, p: &Palette, env: &AskEnv<'_>, card: Rect) -> bool {
        let lift = theme::Elevation {
            y: 2.0,
            blur: 10.0,
            color: p.shadows.popover.color.gamma_multiply(0.5),
            ..p.shadows.panel
        };
        ui.painter().add(lift.drop_shape(card, COMPOSER_RADIUS));
        ui.painter().rect_filled(card, COMPOSER_RADIUS, p.surface);
        ui.painter().rect_stroke(
            card,
            COMPOSER_RADIUS,
            egui::Stroke::new(1.0, p.separator),
            egui::StrokeKind::Inside,
        );
        let row = Rect::from_min_max(
            pos2(card.left() + 10.0, card.bottom() - 8.0 - 32.0),
            pos2(card.right() - 8.0, card.bottom() - 8.0),
        );
        let inner = Rect::from_min_max(
            pos2(card.left() + 16.0, card.top() + 14.0),
            pos2(card.right() - 16.0, row.top() - 8.0),
        );
        let id = egui::Id::new("ask-field");
        // Return sends; shift-return breaks the line.
        let enter =
            ui.memory(|m| m.has_focus(id)) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        let hint = format!("Ask about {}…", env.chapter.display());
        let resp = child.add_sized(
            inner.size(),
            egui::TextEdit::multiline(&mut self.input)
                .id(id)
                .hint_text(hint)
                .font(FontId::new(14.0, theme::family(theme::UI)))
                .text_color(p.ink)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                .desired_rows(1),
        );
        if std::mem::take(&mut self.focus) {
            resp.request_focus();
        }
        // Clicking the card's empty space focuses the field too.
        let bg = ui.interact(card, egui::Id::new("ask-composer"), Sense::click());
        if bg.clicked() {
            resp.request_focus();
        }

        let chip = Rect::from_min_size(pos2(row.left(), row.center().y - 13.0), vec2(0.0, 26.0));
        self.context_chip(ui, p, env, chip);
        let send = Rect::from_center_size(pos2(row.right() - 16.0, row.center().y), vec2(32.0, 32.0));
        let ready = !self.input.trim().is_empty() && self.pending.is_none();
        let sr =
            ui.interact(send, egui::Id::new("ask-send"), if ready { Sense::click() } else { Sense::hover() });
        ui.painter().circle_filled(send.center(), 16.0, if ready { p.ink } else { p.fill_strong });
        let glyph = if ready { p.paper } else { p.surface };
        icons::arrow_up(ui.painter(), Rect::from_center_size(send.center(), vec2(16.0, 16.0)), glyph);
        ready && (enter || sr.on_hover_text("Send (Return)").clicked())
    }

    fn send(&mut self, env: &AskEnv<'_>, context: Option<chat::Context>) {
        let question = self.input.trim().to_string();
        self.input.clear();
        self.messages.push(Message::user(question.clone()));
        self.stick_to_bottom = true;
        let (tx, rx) = channel();
        self.pending = Some(rx);
        let translation = env.library.load(env.translation);
        let chapter = env.chapter;
        let config = self.config.clone();
        self.sermons = Arc::new(transcribe::all(env.data_dir));
        let sermons = self.sermons.clone();
        let conversation = self.conversation.clone();
        std::thread::Builder::new()
            .name("ask".into())
            .spawn(move || {
                let reply = match (translation, config) {
                    (Ok(t), Some(cfg)) => {
                        let tools = chat::Tools { translation: t, sermons: sermons.clone() };
                        let events: Sender<Update> = tx.clone();
                        let mut conversation = conversation.lock().unwrap_or_else(|e| e.into_inner());
                        conversation
                            .ask(&cfg, &tools, context.as_ref(), &question, |e| {
                                let _ = events.send(Update::Event(e));
                            })
                            .unwrap_or_else(|e| {
                                Message::assistant(format!("Linger couldn't get an answer: {e:#}."))
                            })
                    }
                    (Ok(t), None) => {
                        let tools = chat::Tools { translation: t, sermons: sermons.clone() };
                        let context = context.unwrap_or(chat::Context { chapter, selection: None });
                        chat::local_reply(&tools, &context, &question)
                    }
                    (Err(_), _) => Message::assistant(
                        "Linger can only look things up in a downloaded translation. Switch to one of \
                         them to ask about it.",
                    ),
                };
                let _ = tx.send(Update::Done(reply));
            })
            .expect("spawn ask thread");
    }
}

/// A conversation's title: its first question's first line, cut at a word to about 60 characters.
fn title_for(question: &str) -> String {
    let line = question.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("New conversation");
    if line.chars().count() <= 60 {
        return line.to_string();
    }
    let cut: String = line.chars().take(60).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ';', ':', '.', ' ']))
}

/// "Reading John 3", "Reading John 3 · verse 14 selected", "… · verses 14–16 selected".
fn context_label(env: &AskEnv<'_>) -> String {
    let name = env
        .library
        .get(env.translation)
        .map(|e| e.meta.ref_name(env.chapter.book()).to_string())
        .unwrap_or_else(|| env.chapter.book().ref_name().into());
    let mut s = format!("Reading {name} {}", env.chapter.chapter());
    match env.selection {
        Some((a, b)) if a == b => s.push_str(&format!(" · verse {} selected", a.verse)),
        Some((a, b)) if a.chapter == b.chapter => {
            s.push_str(&format!(" · verses {}–{} selected", a.verse, b.verse))
        }
        Some((a, b)) => s.push_str(&format!(" · {} selected", reference::format_range(a, b))),
        None => {}
    }
    s
}

fn user_bubble(ui: &mut Ui, p: &Palette, text: &str, width: f32) {
    let max = width * 0.85;
    let g = layout(ui, text, FontId::new(14.0, theme::family(theme::UI)), p.ink, max - 32.0, 20.0);
    let size = g.size() + vec2(32.0, 20.0);
    let (row, _) = ui.allocate_exact_size(vec2(width + 2.0 * PAD, size.y), Sense::hover());
    let r = Rect::from_min_size(pos2(row.right() - PAD - size.x, row.top()), size);
    // A neutral pill, as in chat apps: the accent stays for what links to the text.
    ui.painter().rect_filled(r, 20.0_f32.min(size.y / 2.0), p.fill);
    ui.painter().galley(r.min + vec2(16.0, 10.0), g, p.ink);
}

/// An answer: paragraphs of reading-face prose; the verses each paragraph cites as chips under it.
fn answer(
    ui: &mut Ui,
    p: &Palette,
    env: &AskEnv<'_>,
    sermons: &[Transcript],
    text: &str,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let font = text::SNIPPET.font();
    for (i, para) in text.split("\n\n").filter(|s| !s.trim().is_empty()).enumerate() {
        if i > 0 {
            ui.add_space(10.0);
        }
        // Citations stay in the sentence as accent-coloured references, and become chips below.
        let mut job = LayoutJob::default();
        let mut refs: Vec<Reference> = Vec::new();
        let format = |color| TextFormat {
            font_id: font.clone(),
            color,
            line_height: Some(24.0),
            ..Default::default()
        };
        let segments = chat::segments(para.trim());
        for seg in &segments {
            match seg {
                Segment::Text(t) => job.append(t, 0.0, format(p.ink)),
                Segment::Verse(r) => {
                    job.append(&ref_label(env, r), 0.0, format(p.accent));
                    if !refs.contains(r) {
                        refs.push(*r);
                    }
                }
                // Named in the sentence; opening the transcript at that moment comes with the
                // sermon library.
                Segment::Sermon { id, at_ms } => {
                    let title = sermons
                        .iter()
                        .find(|t| t.recording_id.starts_with(id.as_str()))
                        .map_or_else(|| "a sermon".to_string(), Transcript::title);
                    job.append(&format!("{title}, {}", transcribe::clock(*at_ms)), 0.0, format(p.accent));
                }
            }
        }
        if !job.text.trim().is_empty() {
            job.wrap.max_width = width;
            let g = ui.fonts_mut(|f| f.layout_job(job));
            let (r, _) = ui.allocate_exact_size(vec2(width + 2.0 * PAD, g.size().y), Sense::hover());
            ui.painter().galley(pos2(r.left() + PAD, r.top()), g, p.ink);
        }
        if !refs.is_empty() {
            ui.add_space(6.0);
            chips(ui, p, env, &refs, width, actions);
        }
    }
}

/// Verse chips, wrapping onto more rows as needed. Clicking one jumps the reader there.
fn chips(
    ui: &mut Ui,
    p: &Palette,
    env: &AskEnv<'_>,
    refs: &[Reference],
    width: f32,
    actions: &mut Vec<Action>,
) {
    let font = text::SMALL.font();
    let h = 24.0;
    let gap = 6.0;
    let labels: Vec<_> =
        refs.iter().map(|r| ui.painter().layout_no_wrap(ref_label(env, r), font.clone(), p.accent)).collect();
    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    let mut x = 0.0;
    for (i, g) in labels.iter().enumerate() {
        let w = 30.0 + g.size().x + 10.0;
        if x > 0.0 && x + w > width {
            rows.push(Vec::new());
            x = 0.0;
        }
        rows.last_mut().expect("one row").push(i);
        x += w + gap;
    }
    for row in rows {
        let (band, _) = ui.allocate_exact_size(vec2(width + 2.0 * PAD, h + gap), Sense::hover());
        let mut x = band.left() + PAD;
        for i in row {
            let g = &labels[i];
            let r = Rect::from_min_size(pos2(x, band.top()), vec2(30.0 + g.size().x + 10.0, h));
            let resp =
                ui.interact(r, egui::Id::new(("ask-chip", ui.id(), band.top() as i32, i)), Sense::click());
            let fill = if resp.hovered() { p.accent.gamma_multiply(0.2) } else { p.accent_soft };
            ui.painter().rect_filled(r, h / 2.0, fill);
            icons::book(
                ui.painter(),
                Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(14.0, 14.0)),
                p.accent,
            );
            ui.painter().galley(pos2(r.left() + 28.0, r.center().y - g.size().y / 2.0), g.clone(), p.accent);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                && let Some(c) = ChapterIdx::new(refs[i].book, refs[i].chapter)
            {
                actions.push(Action::Jump { chapter: c, verse: refs[i].verse, flash: true });
            }
            x = r.right() + gap;
        }
    }
}

/// "John 3:16", "1 Cor 13:4–7", "Psalm 23", with the translation's own book names.
fn ref_label(env: &AskEnv<'_>, r: &Reference) -> String {
    let name = env.library.get(env.translation).map(|e| e.meta.ref_name(r.book).to_string());
    let mut s = format!("{} {}", name.as_deref().unwrap_or(r.book.ref_name()), r.chapter);
    if let Some(v) = r.verse {
        s.push_str(&format!(":{v}"));
        if let Some(e) = r.verse_end.filter(|e| *e > v) {
            s.push_str(&format!("–{e}"));
        }
    }
    s
}

fn layout(
    ui: &Ui,
    s: &str,
    font: FontId,
    color: egui::Color32,
    width: f32,
    line_height: f32,
) -> Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(
        s.to_string(),
        TextFormat { font_id: font, color, line_height: Some(line_height), ..Default::default() },
    );
    job.wrap.max_width = width;
    ui.fonts_mut(|f| f.layout_job(job))
}

fn wrapped_lines(ui: &Ui, s: &str, width: f32) -> usize {
    if s.is_empty() {
        return 1;
    }
    layout(ui, s, FontId::new(14.0, theme::family(theme::UI)), egui::Color32::WHITE, width, 20.0).rows.len()
}
