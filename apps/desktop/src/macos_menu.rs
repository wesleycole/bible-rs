//! Native macOS menu bar.
//!
//! Items with key equivalents are dispatched by AppKit before the key reaches egui, so each menu
//! command is turned back into the same [`Action`] the keyboard path produces. There is no Edit
//! menu on purpose: AppKit's predefined copy/paste items would swallow ⌘C/⌘V before egui's text
//! fields see them (verse copying is ⌘C handled by the app itself).

use muda::accelerator::Accelerator;
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

use crate::actions::{Action, ExportKind, SettingsTab};
use crate::theme::Scheme;

pub struct AppMenu {
    _menu: Menu,
    /// Menu clicks, as muda's handler forwards them.
    events: std::sync::mpsc::Receiver<MenuEvent>,
}

fn item(id: &str, label: &str, accel: Option<&str>) -> MenuItem {
    MenuItem::with_id(id, label, true, accel.and_then(|a| a.parse::<Accelerator>().ok()))
}

impl AppMenu {
    /// Builds the menu bar and installs it as the application menu. A click wakes `ctx`, so it
    /// takes effect at once even when the menu didn't cover the window (nothing else repaints).
    pub fn install(ctx: &eframe::egui::Context) -> Option<Self> {
        let menu = Menu::new();
        let app = Submenu::with_items(
            "Linger",
            true,
            &[
                &PredefinedMenuItem::about(Some("About Linger"), None),
                &PredefinedMenuItem::separator(),
                &item("settings", "Settings…", Some("CmdOrCtrl+Comma")),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::quit(None),
            ],
        )
        .ok()?;
        let file = Submenu::with_items(
            "File",
            true,
            &[
                &item("import_translation", "Import Translation…", Some("CmdOrCtrl+I")),
                &item("translations", "Manage Translations & API Keys…", None),
                &PredefinedMenuItem::separator(),
                &item("export_md", "Export Notes as Markdown…", Some("CmdOrCtrl+E")),
                &item("export_json", "Export Notes as JSON…", Some("CmdOrCtrl+Shift+E")),
                &item("import_notes", "Import Notes from JSON…", None),
            ],
        )
        .ok()?;
        let view = Submenu::with_items(
            "View",
            true,
            &[
                &item("scheme_system", "Match System", None),
                &item("scheme_light", "Light", None),
                &item("scheme_dark", "Dark", None),
                &item("theme_cycle", "Toggle Light and Dark", Some("CmdOrCtrl+Shift+L")),
                &PredefinedMenuItem::separator(),
                &item("bigger", "Bigger Text", Some("CmdOrCtrl+Equal")),
                &item("smaller", "Smaller Text", Some("CmdOrCtrl+Minus")),
                &item("actual", "Actual Size", Some("CmdOrCtrl+Digit0")),
                &item("text", "Text & Layout…", Some("CmdOrCtrl+KeyT")),
                &PredefinedMenuItem::separator(),
                &item("mode", "Verse per Line / Paragraphs", Some("CmdOrCtrl+L")),
                &item("red", "Red Letters", Some("CmdOrCtrl+R")),
                &item("parallel", "Parallel Translations", Some("CmdOrCtrl+Shift+P")),
                &item("sidebar", "Sidebar", Some("CmdOrCtrl+B")),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::fullscreen(None),
            ],
        )
        .ok()?;
        let go = Submenu::with_items(
            "Go",
            true,
            &[
                &item("jump", "Jump to Reference…", Some("CmdOrCtrl+K")),
                &item("search", "Search…", Some("CmdOrCtrl+F")),
                &PredefinedMenuItem::separator(),
                &item("back", "Back", Some("CmdOrCtrl+BracketLeft")),
                &item("forward", "Forward", Some("CmdOrCtrl+BracketRight")),
                &item("prev", "Previous Chapter", None),
                &item("next", "Next Chapter", None),
                &PredefinedMenuItem::separator(),
                &item("bookmark", "Bookmark", Some("CmdOrCtrl+D")),
                &item("compare", "Compare Translations", None),
            ],
        )
        .ok()?;
        let window = Submenu::with_items("Window", true, &[&PredefinedMenuItem::minimize(None)]).ok()?;
        let help = Submenu::with_items(
            "Help",
            true,
            &[&item("shortcuts", "Keyboard Shortcuts", Some("CmdOrCtrl+Slash"))],
        )
        .ok()?;
        menu.append_items(&[&app, &file, &view, &go, &window, &help]).ok()?;
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        let (tx, events) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = tx.send(event);
            ctx.request_repaint();
        }));
        Some(Self { _menu: menu, events })
    }

    /// Drains menu clicks into this frame's actions.
    pub fn poll(&self, actions: &mut Vec<Action>) {
        while let Ok(event) = self.events.try_recv() {
            let a = match event.id.0.as_str() {
                "settings" => Action::OpenSettings(SettingsTab::Reading),
                "import_translation" => Action::ImportTranslation,
                "translations" => Action::OpenSettings(SettingsTab::Translations),
                "export_md" => Action::Export(ExportKind::Markdown),
                "export_json" => Action::Export(ExportKind::Json),
                "import_notes" => Action::ImportStudy,
                "scheme_system" => Action::SetScheme(Scheme::System),
                "scheme_light" => Action::SetScheme(Scheme::Light),
                "scheme_dark" => Action::SetScheme(Scheme::Dark),
                "theme_cycle" => Action::CycleTheme,
                "bigger" => Action::FontSize(1.0),
                "smaller" => Action::FontSize(-1.0),
                "actual" => Action::FontReset,
                "text" => Action::TextFormat,
                "mode" => Action::ToggleMode,
                "red" => Action::ToggleRedLetters,
                "parallel" => Action::ToggleParallel,
                "sidebar" => Action::ToggleSidebar,
                "jump" => Action::OpenPalette,
                "search" => Action::OpenSearch(None),
                "back" => Action::Back,
                "forward" => Action::Forward,
                "prev" => Action::PrevChapter,
                "next" => Action::NextChapter,
                "bookmark" => Action::ToggleBookmark,
                "compare" => Action::Compare,
                "shortcuts" => Action::ShowShortcuts,
                _ => continue,
            };
            actions.push(a);
        }
    }
}
