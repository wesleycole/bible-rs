//! About This Recording: the title, the preacher and where a recording is from, asked for when a
//! recording stops and open again from a sermon Ask cites. Ask attributes what was said by them
//! ("John Piper, of Desiring God") instead of taking every sermon for the user's church's.

use eframe::egui::{self, Order, Rect, Sense, Stroke, Ui, pos2, vec2};

use linger_core::recording::{Details, Manifest, Source};
use linger_core::records::ItemStatus;
use linger_core::transcribe;

use crate::pages::text;
use crate::theme::{self, Palette};
use crate::widgets;

const WIDTH: f32 = 440.0;
const PAD: f32 = 24.0;

pub enum Outcome {
    Open,
    Close,
    /// Save `details` for recording `id`; `mine` says `details.from` is the user's own church.
    Save {
        id: String,
        details: Details,
        mine: bool,
    },
    /// Transcribe recording `id`, `waiting`, in the cloud now.
    TranscribeNow(String),
}

/// What the user's plan lets a `waiting` sermon do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    Free,
    /// Linger Pro, with this month's transcription hours used.
    Pro,
    ProWithHours,
}

/// Where the recording's sermon is in the library, said in a line; `None` until the library has
/// it. A `waiting` sermon says why.
fn status_line(status: Option<ItemStatus>, plan: Plan) -> &'static str {
    match status {
        None => "Only on this Mac until your library syncs.",
        Some(ItemStatus::Recording) => "Recording.",
        Some(ItemStatus::Uploading) => "Uploading to your library…",
        Some(ItemStatus::Waiting) if plan == Plan::ProWithHours => "Waiting to be transcribed.",
        Some(ItemStatus::Waiting) if plan == Plan::Pro => "Waiting until your Pro hours renew.",
        Some(ItemStatus::Waiting) => "Not transcribed. Linger Pro transcribes in the cloud.",
        Some(ItemStatus::Processing | ItemStatus::Review) => "Transcribing…",
        Some(ItemStatus::Ready) => "Transcribed, and in your library.",
        Some(ItemStatus::Error) => "Transcription failed.",
    }
}

pub struct DetailsSheet {
    id: String,
    /// "Oct 19, 2026 · 6:28 · Played on this computer, in Google Chrome"
    about: String,
    /// The title it has without one: "Romans 10 · Oct 19, 2026".
    untitled: String,
    title: String,
    speaker: String,
    from: String,
    app: Option<String>,
    /// `from` is the user's own church.
    mine: bool,
    focus: bool,
}

impl DetailsSheet {
    /// For recording `m`: a microphone recording with nothing said of it yet is taken to be from
    /// the user's church, if they've named it.
    pub fn new(data_dir: &std::path::Path, m: &Manifest, home: Option<&str>) -> Self {
        let d = &m.details;
        let from = match (&d.from, m.source) {
            (Some(from), _) => from.clone(),
            (None, Source::Microphone) => home.unwrap_or_default().to_string(),
            (None, _) => String::new(),
        };
        let heard = match (m.source, &d.app) {
            (Source::Microphone, _) => "Recorded with the microphone".to_string(),
            (Source::SystemAudio, Some(app)) => format!("Played on this computer, in {app}"),
            (Source::SystemAudio, None) => "Played on this computer".to_string(),
            (Source::File, _) => "Imported from a file".to_string(),
        };
        let length = crate::recorder::duration_label(m.duration_ms() / 1000);
        let untitled = match transcribe::load(data_dir, &m.id) {
            Some(mut t) => {
                t.about.details.title = None;
                t.title()
            }
            None => format!("Sermon · {}", transcribe::day(m.started_at)),
        };
        Self {
            id: m.id.clone(),
            about: format!("{} · {length} · {heard}", transcribe::day(m.started_at)),
            untitled,
            title: d.title.clone().unwrap_or_default(),
            speaker: d.speaker.clone().unwrap_or_default(),
            mine: !from.trim().is_empty() && home.is_some_and(|h| h.trim().eq_ignore_ascii_case(from.trim())),
            from,
            app: d.app.clone(),
            focus: true,
        }
    }

    /// `status` is the sermon's in the library now.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        p: &Palette,
        status: Option<ItemStatus>,
        plan: Plan,
        starting: bool,
    ) -> Outcome {
        let screen = ctx.content_rect();
        let mut close = false;
        egui::Area::new(egui::Id::new("details-scrim")).order(Order::Middle).fixed_pos(screen.min).show(
            ctx,
            |ui| {
                let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter().rect_filled(r, 0.0, p.scrim);
                close |= resp.clicked();
            },
        );
        let at = pos2((screen.center().x - WIDTH / 2.0).round(), 96.0);
        let mut outcome = Outcome::Open;
        egui::Area::new(egui::Id::new("details-sheet"))
            .order(Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| outcome = self.sheet(ui, p, at, status, plan, starting));
        if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return Outcome::Close;
        }
        outcome
    }

    fn sheet(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        at: egui::Pos2,
        status: Option<ItemStatus>,
        plan: Plan,
        starting: bool,
    ) -> Outcome {
        let height = 482.0;
        let sheet = Rect::from_min_size(at, vec2(WIDTH, height));
        ui.allocate_rect(sheet, Sense::hover());
        let painter = ui.painter().clone();
        p.shadows.modal.paint(&painter, sheet, 16.0);
        painter.rect_filled(sheet, 16.0, p.surface);
        let x = sheet.left() + PAD;
        let w = WIDTH - 2.0 * PAD;
        text(
            &painter,
            x,
            sheet.top() + 16.0,
            20.0,
            "About This Recording",
            theme::ui_semibold(15.0),
            p.ink,
            false,
        );
        painter.hline(sheet.x_range(), sheet.top() + 51.5, Stroke::new(1.0, p.separator));
        text(&painter, x, sheet.top() + 68.0, 18.0, &self.about, theme::ui_font(13.0), p.ink_2, false);
        let line = if starting { "Starting…" } else { status_line(status, plan) };
        text(&painter, x, sheet.top() + 90.0, 18.0, line, theme::ui_font(13.0), p.ink_3, false);
        // A Pro sermon that waited for hours, now there are some.
        if status == Some(ItemStatus::Waiting) && plan == Plan::ProWithHours && !starting {
            let w = widgets::plain_button_width(ui, "Transcribe Now");
            let r = Rect::from_min_size(pos2(sheet.right() - PAD - w, sheet.top() + 84.0), vec2(w, 28.0));
            if widgets::plain_button(ui, p, r, "Transcribe Now", egui::Id::new("details-transcribe"))
                .clicked()
            {
                return Outcome::TranscribeNow(self.id.clone());
            }
        }

        let mut y = sheet.top() + 130.0;
        let fields: [(&str, &mut String, &str, &str); 3] = [
            ("Title", &mut self.title, &self.untitled, "details-title"),
            ("Preacher", &mut self.speaker, "Who preached", "details-speaker"),
            ("From", &mut self.from, "Church, ministry or channel", "details-from"),
        ];
        for (label, value, placeholder, id) in fields {
            text(&painter, x, y, 16.0, label, theme::ui_semibold(12.0), p.ink_2, false);
            let field = Rect::from_min_size(pos2(x, y + 22.0), vec2(w, 32.0));
            let id = egui::Id::new(id);
            widgets::text_field(ui, p, field, value, placeholder, None, false, false, id);
            y += 72.0;
        }
        if std::mem::take(&mut self.focus) {
            ui.memory_mut(|m| m.request_focus(egui::Id::new("details-title")));
        }

        // Which church is the user's: remembered, so their own recordings fill it in and Ask can
        // tell their church's sermons from others.
        let has_from = !self.from.trim().is_empty();
        let check = Rect::from_min_size(pos2(x, y), vec2(16.0, 16.0));
        if has_from {
            if widgets::checkbox(ui, p, check, self.mine, "This is my church").clicked() {
                self.mine = !self.mine;
            }
        } else {
            self.mine = false;
            let hint = "Name where it's from to mark it as your church.";
            text(&painter, x, y - 1.0, 18.0, hint, theme::ui_font(13.0), p.ink_3, false);
        }
        y += 36.0;
        let note = "Ask uses these to say who preached and where.";
        text(&painter, x, y, 18.0, note, theme::text::SMALL.font(), p.ink_3, false);

        let footer = sheet.bottom() - 64.0;
        painter.hline(sheet.x_range(), footer + 0.5, Stroke::new(1.0, p.separator));
        let cy = footer + 32.0;
        let save_w = widgets::primary_button_width(ui, "Save", false);
        let save = Rect::from_min_size(pos2(sheet.right() - PAD - save_w, cy - 16.0), vec2(save_w, 32.0));
        let skip_w = widgets::plain_button_width(ui, "Not Now");
        let skip = Rect::from_min_size(pos2(save.left() - 10.0 - skip_w, cy - 16.0), vec2(skip_w, 32.0));
        if widgets::plain_button(ui, p, skip, "Not Now", egui::Id::new("details-skip")).clicked() {
            return Outcome::Close;
        }
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if widgets::primary_button(ui, p, save, "Save", None, egui::Id::new("details-save")).clicked()
            || enter
        {
            let details = Details {
                title: Some(self.title.clone()),
                speaker: Some(self.speaker.clone()),
                from: Some(self.from.clone()),
                app: self.app.clone(),
            }
            .cleaned();
            return Outcome::Save { id: self.id.clone(), details, mine: self.mine && has_from };
        }
        Outcome::Open
    }
}
