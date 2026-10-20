//! Ask: a conversation about what is being read, docked right of the reader (concept A).
//!
//! The reader feeds the conversation (the chapter and any selected verses go with each question,
//! shown as a chip that can be dropped) and the conversation points back at the reader: every
//! verse an answer cites is a chip that jumps there, and gets a dot in the reader's margin.
//! Answers come from Claude through `linger_core::assistant`, streamed in as they're written:
//! through Linger's server as the signed-in account, or (development) the Gateway with a key from
//! the environment. Either way the tools read a bundled public-domain translation
//! (`env.ask_translation`), never the reader's, and the header says which.
//!
//! The first use shows a one-time sheet saying what is sent where. A question the server turns
//! down (signed out, the month's questions used, too long) comes back as a notice with what to do
//! about it, and the question goes back into the field.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use eframe::egui::text::LayoutJob;
use eframe::egui::{
    self, Align2, FontId, Key, Modifiers, Rect, ScrollArea, Sense, TextFormat, Ui, pos2, vec2,
};
use linger_core::api::{Api, ApiError, Me};
use linger_core::assistant::{self, Conversation, Event, Mode};
use linger_core::chat::{self, Message, Role, Segment};
use linger_core::reference::{self, Reference};
use linger_core::transcribe::{self, Transcript};
use linger_core::{ChapterIdx, VerseRef};

use crate::actions::{Action, SettingsTab};
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
    /// Development: the Gateway with a key from the environment, used when signed out.
    direct: Option<assistant::Config>,
    /// Ask the next question in Deep study (Pro).
    deep: bool,
    /// Why the last question got no answer, and what to do about it.
    notice: Option<Notice>,
    /// Questions left this month, as the server said with the last answer, and whether that
    /// answer came from a lighter model (past the month's cost ceiling).
    questions_left: Option<u32>,
    downgraded: bool,
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
    /// The reader's translation: citations open in it.
    pub translation: &'a str,
    /// The public-domain translation the tools read and answers quote.
    pub ask_translation: &'a str,
    pub chapter: ChapterIdx,
    pub selection: Option<(VerseRef, VerseRef)>,
    pub data_dir: &'a std::path::Path,
    /// Linger's server as the signed-in account; `None` when signed out.
    pub api: Option<&'a Api>,
    /// The plan and allowance, once loaded.
    pub me: Option<&'a Me>,
    /// The one-time sheet about what Ask sends where was accepted.
    pub consented: bool,
    /// The user's own church, for telling its sermons from others.
    pub church: Option<&'a str>,
}

enum Update {
    Event(Event),
    Done(Message),
    /// No answer: the server's refusal, or another failure's description.
    Failed(Result<ApiError, String>),
}

/// A question that got no answer: why, and a button that does something about it.
#[derive(Debug, Clone, PartialEq)]
struct Notice {
    message: String,
    button: Option<(&'static str, Vec<Action>)>,
}

impl Notice {
    fn new(failure: Result<ApiError, String>, me: Option<&Me>) -> Self {
        let resets = me
            .map(|m| format!(" They reset on {}.", crate::account::day(&m.ask.resets_at)))
            .unwrap_or_default();
        let (message, button) = match failure {
            Ok(ApiError::SignIn(_)) => (
                "Sign in to ask. A free Linger account includes 30 questions a month.".to_string(),
                Some(("Sign In…", vec![Action::SignIn, Action::OpenSettings(SettingsTab::Account)])),
            ),
            Ok(ApiError::Upgrade { message, upgrade, plan_required }) => {
                let message = if plan_required { message } else { format!("{message}{resets}") };
                (message, upgrade.then(|| ("See Pro", vec![Action::OpenSettings(SettingsTab::Account)])))
            }
            Ok(ApiError::Wait { message, retry_after }) => match retry_after {
                Some(s) => (format!("{message} Try again in {}.", wait_label(s)), None),
                None => (message, None),
            },
            Ok(ApiError::Update(_)) => ("Update Linger to keep using Ask.".to_string(), None),
            Ok(ApiError::TooLarge(_)) => (
                "This conversation is too long to carry on. Start a new one to keep asking.".to_string(),
                Some(("New Conversation", vec![Action::NewChat])),
            ),
            Ok(e) => (e.to_string(), None),
            Err(e) => (format!("Linger couldn't get an answer: {e}."), None),
        };
        Self { message, button }
    }
}

/// "40 seconds", "5 minutes", "3 hours".
fn wait_label(seconds: u64) -> String {
    let (n, unit) = match seconds {
        0..=89 => (seconds.max(1), "second"),
        90..=5399 => (seconds.div_ceil(60), "minute"),
        _ => (seconds.div_ceil(3600), "hour"),
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

impl AskPanel {
    pub fn new() -> Self {
        Self { focus: true, direct: assistant::Config::from_env(), ..Default::default() }
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

    /// Reads the sermons again, after their details change.
    pub fn reload_sermons(&mut self, data_dir: &std::path::Path) {
        self.sermons = Arc::new(transcribe::all(data_dir));
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

    /// Screenshots: a question turned down, as `kind` (`signin`, `free`, `pro`, `wait`,
    /// `update`, `long`).
    pub fn demo_notice(&mut self, kind: &str, me: Option<&Me>) {
        let failure = match kind {
            "signin" => ApiError::SignIn(String::new()),
            "free" => ApiError::Upgrade {
                message: "You've used this month's free questions.".into(),
                upgrade: true,
                plan_required: false,
            },
            "pro" => ApiError::Upgrade {
                message: "You've used this month's questions.".into(),
                upgrade: false,
                plan_required: false,
            },
            "deep" => ApiError::Upgrade {
                message: "Deep study is part of Linger Pro.".into(),
                upgrade: true,
                plan_required: true,
            },
            "wait" => ApiError::Wait {
                message: "You've asked as many questions as you can today.".into(),
                retry_after: Some(5 * 3600),
            },
            "update" => ApiError::Update(String::new()),
            _ => ApiError::TooLarge(String::new()),
        };
        if matches!(failure, ApiError::Upgrade { plan_required: false, .. }) {
            self.questions_left = Some(0);
        }
        self.input = "What does it mean to be born of water and the Spirit?".into();
        self.notice = Some(Notice::new(Ok(failure), me));
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
                Update::Event(Event::Allowance { questions_left, downgraded }) => {
                    self.questions_left = questions_left.or(self.questions_left);
                    self.downgraded = downgraded;
                }
                Update::Done(m) => {
                    self.messages.push(m);
                    self.unsaved = true;
                    self.pending = None;
                    self.streaming.clear();
                    self.status = None;
                    if env.api.is_some() {
                        // Settings › Account shows the allowance too.
                        actions.push(Action::RefreshAccount);
                    }
                }
                Update::Failed(failure) => {
                    // The question goes back into the field, to send again once it's sorted.
                    if let Some(m) = self.messages.pop_if(|m| m.role == Role::User)
                        && self.input.trim().is_empty()
                    {
                        self.input = m.text;
                    }
                    // The month's questions are spent: the header shouldn't still count some.
                    if matches!(failure, Ok(ApiError::Upgrade { plan_required: false, .. })) {
                        self.questions_left = Some(0);
                    }
                    self.notice = Some(Notice::new(failure, env.me));
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
        let mut close = self.header(ui, p, env, rect);
        let online = env.api.is_some() || self.direct.is_some();
        if online && !env.consented {
            let body = Rect::from_min_max(pos2(rect.left(), rect.top() + HEADER), rect.max);
            close |= consent(ui, p, body, actions);
            return close;
        }
        let context = (!self.context_off).then(|| chat::Context {
            chapter: env.chapter,
            selection: env.selection,
            reading: abbrev(env, env.translation),
        });

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

    fn header(&mut self, ui: &mut Ui, p: &Palette, env: &AskEnv<'_>, rect: Rect) -> bool {
        let cy = rect.top() + HEADER / 2.0;
        icons::bubble(
            ui.painter(),
            Rect::from_center_size(pos2(rect.left() + PAD + 10.0, cy), vec2(20.0, 20.0)),
            p.accent,
        );
        let title = ui.painter().text(
            pos2(rect.left() + PAD + 28.0, cy),
            Align2::LEFT_CENTER,
            "Ask",
            text::UI_TITLE.font(),
            p.ink,
        );
        // Answers quote a public-domain translation, which may not be the one being read.
        let quotes = format!("Quotes the {}", abbrev(env, env.ask_translation));
        let note = ui.painter().text(
            pos2(title.right() + 8.0, cy + 1.0),
            Align2::LEFT_CENTER,
            quotes,
            text::SMALL.font(),
            p.ink_3,
        );
        let name = env.library.get(env.ask_translation).map(|e| e.meta.name.clone()).unwrap_or_default();
        ui.interact(note, egui::Id::new("ask-quotes"), Sense::hover()).on_hover_text(format!(
            "Ask reads and quotes the {name}, a public-domain translation, whatever you're reading. \
             Citations open in your translation. Change it in Settings › Translations."
        ));
        // What's left of the month's questions, quietly.
        if env.api.is_some()
            && let Some(left) = self.questions_left.or(env.me.map(|m| m.ask.questions_left))
        {
            let r = ui.painter().text(
                pos2(note.right(), cy + 1.0),
                Align2::LEFT_CENTER,
                format!(" · {left} left"),
                text::SMALL.font(),
                p.ink_3,
            );
            let resets = env
                .me
                .map(|m| format!(", until {}", crate::account::day(&m.ask.resets_at)))
                .unwrap_or_default();
            ui.interact(r, egui::Id::new("ask-left"), Sense::hover()).on_hover_text(format!(
                "{left} question{} left this month{resets}.",
                if left == 1 { "" } else { "s" }
            ));
        }
        let mut close = false;
        let close_rect = Rect::from_center_size(pos2(rect.right() - 12.0 - 14.0, cy), vec2(28.0, 28.0));
        if widgets::round_button(ui, p, close_rect, icons::close, egui::Id::new("ask-close"))
            .on_hover_text("Close (⌘J)")
            .clicked()
        {
            close = true;
        }
        if !self.messages.is_empty() || self.notice.is_some() {
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
                self.notice = None;
                self.focus = true;
            }
        }
        close
    }

    fn conversation(&mut self, ui: &mut Ui, p: &Palette, env: &AskEnv<'_>, actions: &mut Vec<Action>) {
        let width = ui.max_rect().width() - 2.0 * PAD;
        if self.messages.is_empty() && self.pending.is_none() && self.notice.is_none() {
            let r = ui.max_rect();
            let signed_out = env.api.is_none() && self.direct.is_none();
            let msg = if signed_out {
                "Ask about what you're reading. Answers cite the verses they draw on. Ask needs a \
                 free Linger account, which includes 30 questions a month."
            } else {
                "Ask about what you're reading. Answers cite the verses they draw on, and \
                 later the sermons you've recorded."
            };
            let g = layout(ui, msg, FontId::new(13.0, theme::family(theme::UI)), p.ink_2, width, 20.0);
            let h = g.size().y + if signed_out { 16.0 + 28.0 } else { 0.0 };
            let top = r.center().y - h / 2.0;
            let gh = g.size().y;
            ui.painter().galley(pos2(r.left() + PAD, top), g, p.ink_2);
            if signed_out {
                let (_, resp) = widgets::Button::new(widgets::Kind::Primary, "Sign In…").small().show_left(
                    ui,
                    p,
                    r.left() + PAD,
                    top + gh + 16.0 + 14.0,
                    egui::Id::new("ask-sign-in"),
                );
                if resp.clicked() {
                    actions.push(Action::SignIn);
                    actions.push(Action::OpenSettings(SettingsTab::Account));
                }
            }
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
            if self.downgraded && self.pending.is_none() && !self.messages.is_empty() {
                let g = layout(
                    ui,
                    "Answered with a lighter model: this month's questions have run long.",
                    text::SMALL.font(),
                    p.ink_3,
                    width,
                    16.0,
                );
                let (r, _) = ui.allocate_exact_size(vec2(width + 2.0 * PAD, g.size().y), Sense::hover());
                ui.painter().galley(pos2(r.left() + PAD, r.top()), g, p.ink_3);
                ui.add_space(12.0);
            }
            if let Some(n) = &self.notice {
                notice_card(ui, p, n, width, actions);
            }
            ui.add_space(8.0);
            if stick {
                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
            }
        });
    }

    /// "Reading John 3 · verse 14 selected", with an × that leaves the next question without it.
    fn context_chip(&mut self, ui: &mut Ui, p: &Palette, env: &AskEnv<'_>, at: Rect, max_right: f32) {
        let label =
            if self.context_off { format!("Add {}", env.chapter.display()) } else { context_label(env) };
        let font = text::SMALL.font();
        let trailing = if self.context_off { 10.0 } else { 26.0 };
        let label = crate::pages::elide(ui.painter(), &label, &font, max_right - at.left() - 10.0 - trailing);
        let g = ui.painter().layout_no_wrap(label, font, if self.context_off { p.ink_2 } else { p.accent });
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

        let send = Rect::from_center_size(pos2(row.right() - 16.0, row.center().y), vec2(32.0, 32.0));
        // Deep study, when the plan has it: a stronger model, counted as two questions.
        let mut chip_right = send.left() - 8.0;
        if env.api.is_some() && env.me.is_some_and(|m| m.ask.deep_available) {
            let font = text::SMALL.font();
            let color = if self.deep { p.accent } else { p.ink_2 };
            let g = ui.painter().layout_no_wrap("Deep study".into(), font, color);
            let r = Rect::from_min_max(
                pos2(chip_right - (g.size().x + 20.0), row.center().y - 13.0),
                pos2(chip_right, row.center().y + 13.0),
            );
            let resp = ui.interact(r, egui::Id::new("ask-deep"), Sense::click());
            let fill = if self.deep {
                p.accent_soft
            } else {
                widgets::hover_fill(ui, &resp, p, false).unwrap_or(p.fill)
            };
            ui.painter().rect_filled(r, r.height() / 2.0, fill);
            ui.painter().galley(pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, color);
            if resp
                .on_hover_text(if self.deep {
                    "Deep study is on: a stronger model for this question, counted as two"
                } else {
                    "Deep study: a stronger model for harder questions, counted as two"
                })
                .clicked()
            {
                self.deep = !self.deep;
            }
            chip_right = r.left() - 8.0;
        }
        let chip = Rect::from_min_size(pos2(row.left(), row.center().y - 13.0), vec2(0.0, 26.0));
        self.context_chip(ui, p, env, chip, chip_right);
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
        self.notice = None;
        // Signed in, through Linger's server; else (development) the Gateway directly.
        let mode = if self.deep && env.me.is_some_and(|m| m.ask.deep_available) {
            Mode::Deep
        } else {
            Mode::Standard
        };
        let Some(config) =
            env.api.map(|api| assistant::Config::proxy(api.clone(), mode)).or_else(|| self.direct.clone())
        else {
            self.notice = Some(Notice::new(Ok(ApiError::SignIn(String::new())), env.me));
            return;
        };
        self.input.clear();
        self.messages.push(Message::user(question.clone()));
        self.stick_to_bottom = true;
        let (tx, rx) = channel();
        self.pending = Some(rx);
        let translation = env.library.load(env.ask_translation);
        self.sermons = Arc::new(transcribe::all(env.data_dir));
        let sermons = self.sermons.clone();
        let church = env.church.map(str::to_string);
        let conversation = self.conversation.clone();
        std::thread::Builder::new()
            .name("ask".into())
            .spawn(move || {
                let update = match translation {
                    Ok(t) => {
                        let tools = chat::Tools { translation: t, sermons, church };
                        let events: Sender<Update> = tx.clone();
                        let mut conversation = conversation.lock().unwrap_or_else(|e| e.into_inner());
                        match conversation.ask(&config, &tools, context.as_ref(), &question, |e| {
                            let _ = events.send(Update::Event(e));
                        }) {
                            Ok(m) => Update::Done(m),
                            Err(e) => Update::Failed(match e.downcast::<ApiError>() {
                                Ok(api) => Ok(api),
                                Err(e) => Err(format!("{e:#}")),
                            }),
                        }
                    }
                    Err(e) => Update::Done(Message::assistant(format!(
                        "Linger couldn't open the Bible Ask reads: {e:#}."
                    ))),
                };
                let _ = tx.send(update);
            })
            .expect("spawn ask thread");
    }
}

/// The one-time sheet before the first question (App Review 5.1.2(i)): what is sent where.
/// Returns `true` for Not Now (close the panel).
fn consent(ui: &mut Ui, p: &Palette, body: Rect, actions: &mut Vec<Action>) -> bool {
    let width = body.width() - 2.0 * PAD - 8.0;
    let left = body.left() + PAD + 4.0;
    let mut y = body.top() + 24.0;
    let title = layout(ui, "Before you ask", text::UI_TITLE.font(), p.ink, width, 24.0);
    let h = title.size().y;
    ui.painter().galley(pos2(left, y), title, p.ink);
    y += h + 12.0;
    let points = [
        "Your questions, and the passages Ask reads to answer them, are sent to Anthropic's Claude \
         through Vercel.",
        "Nothing you send is kept to train models.",
        "Your conversations are saved in your library, in Recents, so they're on all your devices.",
    ];
    for point in points {
        ui.painter().circle_filled(pos2(left + 3.0, y + 10.0), 2.5, p.ink_3);
        let g = layout(ui, point, FontId::new(13.0, theme::family(theme::UI)), p.ink_2, width - 16.0, 20.0);
        let h = g.size().y;
        ui.painter().galley(pos2(left + 16.0, y), g, p.ink_2);
        y += h + 10.0;
    }
    y += 12.0;
    let (b, resp) = widgets::Button::new(widgets::Kind::Primary, "Continue").show_left(
        ui,
        p,
        left,
        y + 16.0,
        egui::Id::new("ask-consent"),
    );
    if resp.clicked() {
        actions.push(Action::AcceptAskConsent);
    }
    let (_, not_now) = widgets::Button::new(widgets::Kind::Plain, "Not Now").show_left(
        ui,
        p,
        b.right() + 8.0,
        y + 16.0,
        egui::Id::new("ask-consent-later"),
    );
    not_now.clicked()
}

/// A question that got no answer: a tinted card with the reason and, if there is one, the way out.
fn notice_card(ui: &mut Ui, p: &Palette, n: &Notice, width: f32, actions: &mut Vec<Action>) {
    let inner = width - 24.0 - 26.0;
    let g = layout(ui, &n.message, FontId::new(13.0, theme::family(theme::UI)), p.ink, inner, 19.0);
    let button_h = if n.button.is_some() { 12.0 + 28.0 } else { 0.0 };
    let h = 12.0 + g.size().y + button_h + 12.0;
    let (row, _) = ui.allocate_exact_size(vec2(width + 2.0 * PAD, h), Sense::hover());
    let card = Rect::from_min_size(pos2(row.left() + PAD, row.top()), vec2(width, h));
    ui.painter().rect_filled(card, 14.0, p.fill);
    icons::info(
        ui.painter(),
        Rect::from_min_size(pos2(card.left() + 12.0, card.top() + 12.0), vec2(18.0, 18.0)),
        p.ink_2,
    );
    let text_left = card.left() + 12.0 + 26.0;
    let gh = g.size().y;
    ui.painter().galley(pos2(text_left, card.top() + 12.0), g, p.ink);
    if let Some((label, todo)) = &n.button {
        let (_, resp) = widgets::Button::new(widgets::Kind::Secondary, label).small().show_left(
            ui,
            p,
            text_left,
            card.top() + 12.0 + gh + 12.0 + 14.0,
            egui::Id::new("ask-notice"),
        );
        if resp.clicked() {
            actions.extend(todo.iter().cloned());
        }
    }
}

/// A translation's abbreviation ("BSB"), or its id in capitals if it isn't installed.
fn abbrev(env: &AskEnv<'_>, id: &str) -> String {
    env.library.get(id).map_or_else(|| id.to_uppercase(), |e| e.meta.abbrev.clone())
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
        let mut heard: Vec<(String, String)> = Vec::new();
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
                // Named in the sentence, with a chip below that shows what's known of the recording
                // (opening the transcript at that moment comes with the sermon library).
                Segment::Sermon { id, at_ms } => {
                    let sermon = sermons.iter().find(|t| t.recording_id.starts_with(id.as_str()));
                    let title = sermon.map_or_else(|| "a sermon".to_string(), Transcript::title);
                    job.append(&format!("{title}, {}", transcribe::clock(*at_ms)), 0.0, format(p.accent));
                    if let Some(t) = sermon
                        && !heard.iter().any(|(id, _)| *id == t.recording_id)
                    {
                        heard.push((t.recording_id.clone(), title));
                    }
                }
            }
        }
        if !job.text.trim().is_empty() {
            job.wrap.max_width = width;
            let g = ui.fonts_mut(|f| f.layout_job(job));
            let (r, _) = ui.allocate_exact_size(vec2(width + 2.0 * PAD, g.size().y), Sense::hover());
            ui.painter().galley(pos2(r.left() + PAD, r.top()), g, p.ink);
        }
        let mut list: Vec<Chip> = refs
            .iter()
            .filter_map(|r| {
                let chapter = ChapterIdx::new(r.book, r.chapter)?;
                let jump = Action::Jump { chapter, verse: r.verse, flash: true };
                Some(Chip { label: ref_label(env, r), icon: icons::book, action: jump, tip: None })
            })
            .collect();
        list.extend(heard.into_iter().map(|(id, title)| Chip {
            label: title,
            icon: icons::mic,
            action: Action::EditRecording(id),
            tip: Some("Who preached and where"),
        }));
        if !list.is_empty() {
            ui.add_space(6.0);
            chips(ui, p, &list, width, actions);
        }
    }
}

/// A chip under an answer: a verse it cites, which jumps the reader there, or a sermon, which opens
/// what's known of the recording.
struct Chip {
    label: String,
    icon: crate::widgets::IconFn,
    action: Action,
    tip: Option<&'static str>,
}

/// Chips, wrapping onto more rows as needed.
fn chips(ui: &mut Ui, p: &Palette, list: &[Chip], width: f32, actions: &mut Vec<Action>) {
    let font = text::SMALL.font();
    let h = 24.0;
    let gap = 6.0;
    let labels: Vec<_> =
        list.iter().map(|c| ui.painter().layout_no_wrap(c.label.clone(), font.clone(), p.accent)).collect();
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
            (list[i].icon)(
                ui.painter(),
                Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(14.0, 14.0)),
                p.accent,
            );
            ui.painter().galley(pos2(r.left() + 28.0, r.center().y - g.size().y / 2.0), g.clone(), p.accent);
            let mut resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            if let Some(tip) = list[i].tip {
                resp = resp.on_hover_text(tip);
            }
            if resp.clicked() {
                actions.push(list[i].action.clone());
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
