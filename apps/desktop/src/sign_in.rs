//! The sign-in screen. Linger needs an account, so a device that isn't signed in sees this, in the
//! onboarding window, until it is: before onboarding on a first launch, and after signing out or a
//! session ending. Whether the device is signed in is decided from the session kept on it, so
//! reading works offline once it is.

use eframe::egui::{self, Rect, Ui, pos2, vec2};

use crate::account::Account;
use crate::actions::Action;
use crate::onboarding::{app_icon, centered, display, palette};
use crate::theme;
use crate::widgets;

#[derive(Default)]
pub struct SignIn {
    /// Development builds: a pasted session token.
    token: String,
}

impl SignIn {
    /// Draws the screen over `ui`'s whole rectangle.
    pub fn show(&mut self, ui: &mut Ui, account: &Account, actions: &mut Vec<Action>) {
        let p = palette(ui.ctx());
        let full = ui.max_rect();
        let painter = ui.painter().clone();
        painter.rect_filled(full, 0.0, p.paper);
        let cx = full.center().x;
        // The icon, title, text and button (about 340 pt) centred, a little above the middle.
        let top = full.center().y - 200.0;

        app_icon(&painter, Rect::from_center_size(pos2(cx, top + 32.0), vec2(64.0, 64.0)));
        centered(&painter, cx, top + 96.0, 48.0, "Sign in to Linger", display(40.0), p.ink, -0.015);
        for (i, s) in [
            "Your account keeps your plan, your questions and your sermon transcripts.",
            "Sign in once on this computer; reading works offline after that.",
        ]
        .iter()
        .enumerate()
        {
            centered(
                &painter,
                cx,
                top + 156.0 + 24.0 * i as f32,
                24.0,
                s,
                theme::ui_font(15.0),
                p.ink_2,
                0.0,
            );
        }

        let button_top = top + 236.0;
        if let Some(busy) = account.busy {
            centered(&painter, cx, button_top, 32.0, busy, theme::ui_font(14.0), p.ink_2, 0.0);
            let w = widgets::plain_button_width(ui, "Cancel");
            let r = Rect::from_min_size(pos2(cx - w / 2.0, button_top + 44.0), vec2(w, 32.0));
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if widgets::plain_button(ui, p, r, "Cancel", ui.id().with("sign-in-cancel")).clicked() || escape {
                actions.push(Action::CancelSignIn);
            }
        } else {
            let w = widgets::primary_button_width(ui, "Sign In", true);
            let r = Rect::from_min_size(pos2(cx - w / 2.0, button_top), vec2(w, 32.0));
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter)) && ui.memory(|m| m.focused().is_none());
            if widgets::primary_button(
                ui,
                p,
                r,
                "Sign In",
                Some(crate::icons::arrow_right),
                ui.id().with("sign-in"),
            )
            .clicked()
                || enter
            {
                actions.push(Action::SignIn);
            }
            centered(
                &painter,
                cx,
                button_top + 44.0,
                20.0,
                "Opens your browser to sign in with Apple, Google or your email.",
                theme::text::SMALL.font(),
                p.ink_3,
                0.0,
            );
        }
        if let Some(error) = &account.error {
            centered(&painter, cx, button_top + 92.0, 20.0, error, theme::ui_font(13.0), p.danger, 0.0);
        }

        if cfg!(debug_assertions) {
            // Development: sign in with a token from `scripts/dev-token.ts`, without Clerk.
            let cy = full.bottom() - 40.0;
            let (bw, fw) = (widgets::plain_button_width(ui, "Use Token"), 420.0);
            let left = cx - (fw + 10.0 + bw) / 2.0;
            let field = Rect::from_min_size(pos2(left, cy - 15.0), vec2(fw, 30.0));
            crate::settings_ui::key_field(
                ui,
                p,
                field,
                &mut self.token,
                "Development: a Clerk session token",
                ui.id().with("sign-in-token"),
            );
            let r = Rect::from_min_size(pos2(field.right() + 10.0, cy - 16.0), vec2(bw, 32.0));
            if widgets::plain_button(ui, p, r, "Use Token", ui.id().with("sign-in-use-token")).clicked()
                && !self.token.trim().is_empty()
            {
                actions.push(Action::SetDevToken(std::mem::take(&mut self.token)));
            }
        }
    }
}
