//! The library consent sheet (docs/plans/backend.md, "Privacy and consent"): shown once, signed in,
//! before the library syncs. Accepting starts sync; "Keep on This Mac" (\"…Computer\" off macOS) leaves the library here,
//! unsynced, until the user turns sync on from the sidebar footer, which shows the sheet again.

use eframe::egui::{self, FontId, Order, Rect, Sense, Stroke, pos2, vec2};

use crate::pages::text;
use crate::theme::{self, Palette};
use crate::widgets;

const WIDTH: f32 = 460.0;
const PAD: f32 = 24.0;

pub enum Outcome {
    Open,
    Accept,
    Decline,
}

use linger_core::privacy::{LIBRARY_CONSENT_POINTS as POINTS, LIBRARY_CONSENT_TITLE};

pub fn show(ctx: &egui::Context, p: &Palette) -> Outcome {
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("consent-scrim")).order(Order::Middle).fixed_pos(screen.min).show(
        ctx,
        |ui| {
            // The scrim takes clicks without closing: the sheet needs an answer.
            let (r, _) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, p.scrim);
        },
    );
    let at = pos2((screen.center().x - WIDTH / 2.0).round(), 96.0);
    let mut outcome = Outcome::Open;
    egui::Area::new(egui::Id::new("consent-sheet")).order(Order::Foreground).fixed_pos(at).show(ctx, |ui| {
        let painter = ui.painter().clone();
        let w = WIDTH - 2.0 * PAD;
        let x = at.x + PAD;
        // Laid out first, so the sheet is as tall as its text.
        let body = FontId::new(13.0, theme::family(theme::UI));
        let galleys: Vec<_> =
            POINTS.iter().map(|t| painter.layout(t.to_string(), body.clone(), p.ink_2, w - 16.0)).collect();
        let points_h: f32 = galleys.iter().map(|g| g.size().y + 12.0).sum();
        let height = 52.0 + 20.0 + points_h + 8.0 + 64.0;
        let sheet = Rect::from_min_size(at, vec2(WIDTH, height));
        ui.allocate_rect(sheet, Sense::hover());
        p.shadows.modal.paint(&painter, sheet, 16.0);
        painter.rect_filled(sheet, 16.0, p.surface);
        text(
            &painter,
            x,
            sheet.top() + 16.0,
            20.0,
            LIBRARY_CONSENT_TITLE,
            theme::ui_semibold(15.0),
            p.ink,
            false,
        );
        painter.hline(sheet.x_range(), sheet.top() + 51.5, Stroke::new(1.0, p.separator));
        let mut y = sheet.top() + 72.0;
        for g in galleys {
            painter.circle_filled(pos2(x + 3.0, y + 9.0), 2.5, p.ink_3);
            let h = g.size().y;
            painter.galley(pos2(x + 16.0, y), g, p.ink_2);
            y += h + 12.0;
        }
        let footer = sheet.bottom() - 64.0;
        painter.hline(sheet.x_range(), footer + 0.5, Stroke::new(1.0, p.separator));
        let cy = footer + 32.0;
        let ok_label = "Turn On Sync";
        let ok_w = widgets::primary_button_width(ui, ok_label, false);
        let ok = Rect::from_min_size(pos2(sheet.right() - PAD - ok_w, cy - 16.0), vec2(ok_w, 32.0));
        let no_label = if cfg!(target_os = "macos") { "Keep on This Mac" } else { "Keep on This Computer" };
        let no_w = widgets::plain_button_width(ui, no_label);
        let no = Rect::from_min_size(pos2(ok.left() - 10.0 - no_w, cy - 16.0), vec2(no_w, 32.0));
        if widgets::plain_button(ui, p, no, no_label, egui::Id::new("consent-decline")).clicked() {
            outcome = Outcome::Decline;
        }
        if widgets::primary_button(ui, p, ok, ok_label, None, egui::Id::new("consent-accept")).clicked() {
            outcome = Outcome::Accept;
        }
    });
    outcome
}
