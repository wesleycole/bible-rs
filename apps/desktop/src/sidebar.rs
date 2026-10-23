//! Left sidebar: a floating source list (study collections, history, reading plans, then Recents:
//! the saved Ask conversations) that turns into search results while a search is active. Books
//! are chosen from the toolbar's picker and translations in Settings.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, Painter, Rect, Response, ScrollArea, Sense, Ui, pos2, vec2,
};
use linger_core::books::{BookId, ChapterIdx, FIRST_NT_BOOK, VerseRef};
use linger_core::library::Library;
use linger_core::search::{Query, Scope};
use linger_core::study::{HighlightColor, StudyStore};
use linger_core::sync::SyncStatus;

use crate::actions::{Action, ExportKind, SidebarTab};
use crate::theme::{self, Palette, Theme, text};
use crate::widgets;

/// Source-list geometry (points): rows inset from the panel's edges, 32 pt rows on 8 pt padding.
const LIST_INSET: f32 = 8.0;
const ROW_HEIGHT: f32 = 32.0;
const ROW_PAD: f32 = 8.0;
const ICON_LANE: f32 = 16.0;
const GAP: f32 = 8.0;
/// Book rows indent their labels to line up with the labels of icon rows.
/// Chapter chips: 24 pt circles, 8 pt apart, seven to a row, indented 24 pt.
/// An open collection lists its most recent items, then "Show all".
const RECENT_ITEMS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StudyKind {
    Bookmarks,
    Highlights,
}

impl StudyKind {
    const ALL: [StudyKind; 2] = [StudyKind::Bookmarks, StudyKind::Highlights];
}

/// What the sidebar needs from the rest of the app each frame.
pub struct SidebarEnv<'a> {
    pub library: &'a Library,
    pub study: &'a StudyStore,
    pub translation: &'a str,
    pub theme: Theme,
    pub location: (ChapterIdx, u8),
    /// The verse selected in the reader (its list item shows as selected).
    pub selection: Option<VerseRef>,
    /// The page in the main area: its row is selected.
    pub page: crate::pages::Page,
    /// The saved conversation open in Ask: its row in Recents is selected.
    pub chat: Option<i64>,
}

pub struct Sidebar {
    /// Recents (saved Ask conversations): shown or folded, and the list as of the study store's
    /// generation.
    recents_open: bool,
    chats: (u64, Vec<linger_core::study::ChatSummary>),
    /// Which study collections are expanded, and the one last opened (drawn selected).
    study_open: [bool; 2],
    study_selected: Option<StudyKind>,
    /// Collections showing every item rather than the most recent few.
    study_all: [bool; 2],
    /// Highlights shown: all, or one colour.
    highlight_filter: Option<HighlightColor>,
    pub query: String,
    /// Whether the toolbar's search field has focus (arrow keys then move through results).
    pub field_focused: bool,
    focus: bool,
    scope: Scope,
    results: Arc<Vec<u32>>,
    /// The result list laid out (headers and hits with their positions).
    entries: Arc<Vec<Entry>>,
    key: Option<(String, String, Scope, bool)>,
    /// Results ordered with the current book first, and the book they were ordered for.
    current_first: bool,
    order_key: (bool, BookId),
    /// Steps through the results asked for by the reader's navigator.
    pending_step: i32,
    elapsed: Duration,
    selected: usize,
    scroll_to_selected: bool,
    /// For a search that found nothing: a spelling that finds verses (query, count, first verse).
    suggestion: Option<(String, usize, u32)>,
}

impl Sidebar {
    pub fn new(_tab: SidebarTab) -> Self {
        Self {
            recents_open: true,
            chats: (0, Vec::new()),
            study_open: [false; 2],
            study_selected: None,
            study_all: [false; 2],
            highlight_filter: None,
            query: String::new(),
            field_focused: false,
            focus: false,
            scope: Scope::All,
            results: Arc::new(Vec::new()),
            entries: Arc::new(Vec::new()),
            key: None,
            current_first: false,
            order_key: (false, BookId(0)),
            pending_step: 0,
            elapsed: Duration::ZERO,
            selected: 0,
            scroll_to_selected: false,
            suggestion: None,
        }
    }

    pub fn focus_search(&mut self, q: Option<String>) {
        self.focus = true;
        if let Some(q) = q {
            self.query = q;
        }
    }

    /// Whether the toolbar's search field should take focus this frame.
    pub fn take_focus(&mut self) -> bool {
        std::mem::take(&mut self.focus)
    }

    /// Recents: the Ask conversations, newest first, under a header that folds them and starts a
    /// new one.
    fn recents(&mut self, ui: &mut Ui, env: &SidebarEnv<'_>, actions: &mut Vec<Action>) {
        let p = env.theme.palette();
        if self.chats.0 != env.study.generation() {
            self.chats = (env.study.generation(), env.study.chats());
        }
        // The header: "Recents" and a chevron fold the list; the pen starts a conversation. The
        // chevron and pen show only while the pointer is over the header.
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
        let cy = rect.top() + 16.0 + 14.0;
        let hover = ui.rect_contains_pointer(Rect::from_min_max(pos2(rect.left(), cy - 16.0), rect.max));
        let label = ui.painter().layout_no_wrap("Recents".into(), text::CAPTION.font(), p.ink_3);
        let chevron = Rect::from_center_size(
            pos2(rect.left() + ROW_PAD + label.size().x + 4.0 + 7.0, cy),
            vec2(14.0, 14.0),
        );
        let fold = Rect::from_min_max(pos2(rect.left(), cy - 14.0), pos2(chevron.right() + 6.0, cy + 14.0));
        let fold_resp = ui.interact(fold, ui.id().with("recents-fold"), Sense::click());
        let ink = if fold_resp.hovered() { p.ink_2 } else { p.ink_3 };
        ui.painter().galley(pos2(rect.left() + ROW_PAD, cy - label.size().y / 2.0), label, ink);
        if hover {
            if self.recents_open {
                crate::icons::chevron_down(ui.painter(), chevron, ink);
            } else {
                crate::icons::disclosure(ui.painter(), chevron, ink);
            }
        }
        if fold_resp.clicked() {
            self.recents_open = !self.recents_open;
        }
        if hover {
            let b = Rect::from_center_size(pos2(rect.right() - ROW_PAD - 6.0, cy), vec2(28.0, 28.0));
            let resp = ui.interact(b, ui.id().with("recents-new"), Sense::click());
            if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
                ui.painter().rect_filled(b, theme::radius::SM, fill);
            }
            let icon = Rect::from_center_size(b.center(), vec2(18.0, 18.0));
            crate::icons::compose(ui.painter(), icon, if resp.hovered() { p.ink } else { p.ink_2 });
            if resp.on_hover_text("New conversation").clicked() {
                actions.push(Action::NewChat);
            }
        }
        if !self.recents_open {
            return;
        }
        if self.chats.1.is_empty() {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
            ui.painter().text(
                pos2(r.left() + ROW_PAD, r.center().y),
                Align2::LEFT_CENTER,
                "Conversations with Ask show here",
                text::SMALL.font(),
                p.ink_3,
            );
            return;
        }
        for chat in &self.chats.1 {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
            let selected = env.chat == Some(chat.id);
            if selected {
                ui.painter().rect_filled(rect, theme::radius::SM, p.accent_soft);
            } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
                ui.painter().rect_filled(rect, theme::radius::SM, fill);
            }
            // One line, cut with an ellipsis.
            let (font, color) =
                if selected { (text::UI_STRONG.font(), p.accent) } else { (text::UI.font(), p.ink) };
            let mut job = egui::text::LayoutJob::single_section(
                chat.title.clone(),
                egui::TextFormat { font_id: font, color, ..Default::default() },
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 2.0 * ROW_PAD);
            let g = ui.painter().layout_job(job);
            ui.painter().galley(pos2(rect.left() + ROW_PAD, rect.center().y - g.size().y / 2.0), g, color);
            if resp.clicked() && !selected {
                actions.push(Action::OpenChat(chat.id));
            }
            resp.context_menu(|ui| {
                if ui.button("Delete Conversation").clicked() {
                    actions.push(Action::DeleteChat(chat.id));
                    ui.close();
                }
            });
        }
    }

    /// Narrows the search to a testament (`ot`, `nt`) or the whole Bible.
    pub fn set_scope(&mut self, name: &str) {
        self.scope = match name {
            "ot" => Scope::OldTestament,
            "nt" => Scope::NewTestament,
            _ => Scope::All,
        };
    }

    /// Expands a study collection by name (`bookmarks`, `highlights`).
    pub fn open_study(&mut self, name: &str) {
        let kind = match name {
            "bookmarks" => StudyKind::Bookmarks,
            "highlights" => StudyKind::Highlights,
            _ => return,
        };
        self.toggle_study(kind);
    }

    fn toggle_study(&mut self, kind: StudyKind) {
        let i = StudyKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
        self.study_open[i] = !self.study_open[i];
        self.study_selected = if self.study_open[i] {
            Some(kind)
        } else {
            StudyKind::ALL.iter().zip(self.study_open).rev().find(|(_, o)| *o).map(|(k, _)| *k)
        };
    }

    pub fn invalidate_search(&mut self) {
        self.key = None;
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// The selected result and how many there are, while a search has results.
    pub fn result_position(&self) -> Option<(usize, usize)> {
        (self.searching() && !self.results.is_empty()).then(|| (self.selected + 1, self.results.len()))
    }

    /// Moves to the next (`1`) or previous (`-1`) result.
    pub fn step_result(&mut self, delta: i32) {
        self.pending_step += delta;
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        env: &SidebarEnv<'_>,
        whole_words: &mut bool,
        actions: &mut Vec<Action>,
    ) {
        if self.searching() {
            self.search(ui, env.library, env.translation, whole_words, env.theme, env.location, actions);
        } else {
            self.source_list(ui, env, actions);
        }
    }

    /// The source list: the study collections and pages. Books are chosen from the toolbar's
    /// picker, translations in Settings.
    fn source_list(&mut self, ui: &mut Ui, env: &SidebarEnv<'_>, actions: &mut Vec<Action>) {
        let p = env.theme.palette();
        let study = env.study;
        let (nh, _, nb) = study.counts();
        ScrollArea::vertical().auto_shrink(false).id_salt("source-list").show(ui, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 8 }).show(
                ui,
                |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    section(ui, p, "Study", true);
                    use crate::pages::Page;
                    for (i, (kind, label, n, icon, page)) in [
                        (
                            StudyKind::Bookmarks,
                            "Bookmarks",
                            nb,
                            icon_bookmark as IconFn,
                            Some(Page::Bookmarks),
                        ),
                        (
                            StudyKind::Highlights,
                            "Highlights",
                            nh,
                            crate::icons::highlighter,
                            Some(Page::Highlights),
                        ),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let open = self.study_open[i];
                        // Rows open their page; the chevron (shown on hover, or while open)
                        // lists the most recent items in place.
                        let next =
                            Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), ROW_HEIGHT));
                        let hovered = ui.rect_contains_pointer(next);
                        let selected = match page {
                            Some(pg) => env.page == pg,
                            None => self.study_selected == Some(kind),
                        };
                        let r = row(
                            ui,
                            p,
                            &Row {
                                icon: Some(icon),
                                label,
                                count: (n > 0).then(|| n.to_string()),
                                disclosure: (hovered || open).then_some(open),
                                selected,
                                ..Row::default()
                            },
                        );
                        let on_chevron = r
                            .interact_pointer_pos()
                            .is_some_and(|pos| pos.x > r.rect.right() - ROW_PAD - ICON_LANE - 4.0);
                        if r.clicked() {
                            match page {
                                Some(pg) if !on_chevron => actions.push(Action::OpenPage(pg)),
                                _ => self.toggle_study(kind),
                            }
                        }
                        if open {
                            self.study_items(ui, env, kind, actions);
                        }
                    }
                    let items = study.item_count();
                    let r = row(
                        ui,
                        p,
                        &Row {
                            icon: Some(crate::icons::folder),
                            label: "Library",
                            count: (items > 0).then(|| items.to_string()),
                            selected: env.page == crate::pages::Page::Library,
                            ..Row::default()
                        },
                    );
                    if r.clicked() {
                        actions.push(Action::OpenPage(crate::pages::Page::Library));
                    }
                    let r = row(
                        ui,
                        p,
                        &Row {
                            icon: Some(crate::icons::history),
                            label: "History",
                            selected: env.page == crate::pages::Page::History,
                            ..Row::default()
                        },
                    );
                    if r.clicked() {
                        actions.push(Action::OpenPage(crate::pages::Page::History));
                    }
                    let active = study.plans().iter().filter(|p| p.finished.is_none()).count();
                    let r = row(
                        ui,
                        p,
                        &Row {
                            icon: Some(crate::icons::checklist),
                            label: "Reading Plans",
                            count: (active > 0).then(|| active.to_string()),
                            selected: env.page == crate::pages::Page::Plans,
                            ..Row::default()
                        },
                    );
                    if r.clicked() {
                        actions.push(Action::OpenPage(crate::pages::Page::Plans));
                    }
                    self.recents(ui, env, actions);
                },
            );
        });
    }

    /// A study collection's items (`ds-09` list items), newest first.
    fn study_items(&mut self, ui: &mut Ui, env: &SidebarEnv<'_>, kind: StudyKind, actions: &mut Vec<Action>) {
        let p = env.theme.palette();
        let study = env.study;
        let t = env.library.load(env.translation).ok();
        let verse_text =
            |v: VerseRef| t.as_ref().and_then(|t| t.find(v)).map(|v| v.plain_text()).unwrap_or_default();
        // (verse, colour bar, note)
        let items: Vec<(VerseRef, Option<Color32>, Option<String>)> = match kind {
            StudyKind::Bookmarks => {
                let mut b = study.bookmarks();
                b.sort_by_key(|(v, b)| (std::cmp::Reverse(b.created), *v));
                b.into_iter().map(|(v, _)| (v, None, None)).collect()
            }
            StudyKind::Highlights => {
                colour_filter(ui, p, &mut self.highlight_filter);
                study
                    .highlights_newest_first()
                    .into_iter()
                    .filter(|(_, c)| self.highlight_filter.is_none_or(|f| f == *c))
                    .map(|(v, c)| (v, Some(p.swatch(c)), None))
                    .collect()
            }
        };
        if items.is_empty() {
            let hint = match kind {
                StudyKind::Bookmarks => "No bookmarks yet. Select a verse and press B (or ⌘D).",
                StudyKind::Highlights => "No highlights yet. Select a verse and press 1–6.",
            };
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.label(egui::RichText::new(hint).font(text::SMALL.font()).color(p.ink_2));
            });
            return;
        }
        let abbrev = t.as_ref().map(|t| t.meta.abbrev.clone()).unwrap_or_default();
        let k = StudyKind::ALL.iter().position(|x| *x == kind).unwrap_or(0);
        let total = items.len();
        let shown = if self.study_all[k] { total } else { total.min(RECENT_ITEMS) };
        for (v, bar, note) in items.into_iter().take(shown) {
            let name = t
                .as_ref()
                .map(|t| t.meta.ref_name(v.book).to_string())
                .unwrap_or_else(|| v.book.ref_name().into());
            let item = ListItem {
                reference: format!("{name} {}:{}", v.chapter, v.verse),
                trailing: abbrev.clone(),
                bar,
                note,
                snippet: verse_text(v),
                selected: env.selection == Some(v),
            };
            let resp = list_item(ui, p, &item);
            if resp.clicked() {
                actions.push(Action::Jump { chapter: v.chapter_idx(), verse: Some(v.verse), flash: true });
            }
            resp.context_menu(|ui| {
                let (label, a) = match kind {
                    StudyKind::Bookmarks => ("Remove bookmark", Action::RemoveBookmark(v)),
                    StudyKind::Highlights => ("Remove highlight", Action::RemoveHighlight(v)),
                };
                if ui.button(label).clicked() {
                    actions.push(a);
                    ui.close();
                }
                ui.separator();
                if ui.button("Export as Markdown…").clicked() {
                    actions.push(Action::Export(ExportKind::Markdown));
                    ui.close();
                }
                if ui.button("Export as JSON…").clicked() {
                    actions.push(Action::Export(ExportKind::Json));
                    ui.close();
                }
                if ui.button("Import…").clicked() {
                    actions.push(Action::ImportStudy);
                    ui.close();
                }
            });
        }
        if shown < total {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
            if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
                ui.painter().rect_filled(rect, theme::radius::SM, fill);
            }
            ui.painter().text(
                rect.left_center() + vec2(ITEM_PAD.x, 0.0),
                Align2::LEFT_CENTER,
                format!("Show all {total}"),
                theme::ui_medium(text::SMALL.size),
                p.accent,
            );
            if resp.clicked() {
                self.study_all[k] = true;
            }
        }
    }

    /// A search that found nothing (frame 32): why, ways to widen it, and a spelling that finds
    /// something, optically centred in the panel under the count line.
    fn no_results(&mut self, ui: &mut Ui, p: &Palette, t: &linger_core::Translation, whole_words: &mut bool) {
        let filtered = self.scope != Scope::All || *whole_words;
        let several = self.query.split_whitespace().count() > 1;
        let body = if several {
            "Every word has to appear in the same verse.\nTry fewer words or search the whole Bible."
        } else {
            "Check the spelling, or try another word\nor the whole Bible."
        };
        let title = format!("No results for “{}”", self.query.trim());
        let scoped = self.scope != Scope::All;
        let mut buttons = Vec::new();
        if scoped {
            buttons.push(widgets::Button::new(widgets::Kind::Secondary, "Search Whole Bible"));
        }
        if filtered {
            buttons.push(widgets::Button::new(widgets::Kind::Ghost, "Clear filters"));
        }
        let empty = crate::pages::Empty {
            glyph: crate::icons::search_thin,
            tone: crate::pages::Tone::Quiet,
            title: &title,
            body,
            buttons: &buttons,
            hints: &[],
            width: 320.0,
        };
        // It scrolls when the panel is too short for it.
        ScrollArea::vertical().id_salt("no-results").auto_shrink(false).show(ui, |ui| {
            // The viewport, in the panel's scrolling coordinates.
            let area = ui.max_rect();
            let cx = area.center().x;
            let painter = ui.painter().clone();
            let room = area.width();
            let state_h = empty.height(&painter, room);
            let h = state_h + if self.suggestion.is_some() { 20.0 + 60.0 } else { 0.0 };
            let top = crate::pages::optical_top(area.y_range(), h, area.top() + 24.0);
            let responses = empty.show(ui, p, cx, top, room);
            let mut responses = responses.iter();
            if scoped && responses.next().is_some_and(|r| r.clicked()) {
                self.scope = Scope::All;
            }
            if filtered && responses.next().is_some_and(|r| r.clicked()) {
                self.scope = Scope::All;
                *whole_words = false;
            }
            let mut y = top + state_h;
            // A spelling that finds verses.
            if let Some((q, n, first)) = self.suggestion.clone() {
                y += 20.0;
                let suggestion = format!("“{q}”");
                let reference = t.verse_view(first).map(|v| v.reference().to_string()).unwrap_or_default();
                let detail = if n == 1 {
                    format!("1 verse · {reference}")
                } else {
                    format!("{} verses · first {reference}", group(n))
                };
                let sw = painter
                    .layout_no_wrap(suggestion.clone(), theme::ui_semibold(13.0), Color32::PLACEHOLDER)
                    .size()
                    .x;
                let dw =
                    painter.layout_no_wrap(detail.clone(), text::SMALL.font(), Color32::PLACEHOLDER).size().x;
                let inner = (sw + 8.0 + dw).max(80.0);
                let card = Rect::from_min_size(pos2(cx - inner / 2.0 - 16.0, y), vec2(inner + 32.0, 60.0));
                let resp = ui.interact(card, ui.id().with("did-you-mean"), Sense::click());
                painter.rect_filled(card, 12.0, if resp.hovered() { p.fill_strong } else { p.fill });
                painter.text(
                    pos2(cx, y + 12.0 + 8.0),
                    Align2::CENTER_CENTER,
                    "Did you mean",
                    theme::ui_medium(12.0),
                    p.ink_3,
                );
                let x = cx - inner / 2.0;
                painter.text(
                    pos2(x, y + 32.0 + 8.0),
                    Align2::LEFT_CENTER,
                    &suggestion,
                    theme::ui_semibold(13.0),
                    p.accent,
                );
                painter.text(
                    pos2(x + sw + 8.0, y + 32.0 + 8.0),
                    Align2::LEFT_CENTER,
                    detail,
                    text::SMALL.font(),
                    p.ink_3,
                );
                if resp.clicked() {
                    self.query = q;
                }
            }
            ui.allocate_rect(
                Rect::from_min_max(area.min, pos2(area.right(), top + h + 16.0)),
                Sense::hover(),
            );
        });
    }

    /// Search results (frame 06 in the design): scope, book and whole-word filters, a count, then
    /// the hits grouped by book with the matched phrase marked.
    #[allow(clippy::too_many_arguments)]
    fn search(
        &mut self,
        ui: &mut Ui,
        library: &Library,
        translation: &str,
        whole_words: &mut bool,
        theme: Theme,
        location: (ChapterIdx, u8),
        actions: &mut Vec<Action>,
    ) {
        let p = theme.palette();
        let Ok(t) = library.load(translation) else {
            ui.label(
                "Search is available for local translations. Online translations are fetched per chapter.",
            );
            return;
        };
        let mut jump_to: Option<u32> = None;
        // The query lives in the toolbar's field; arrow keys and return drive the results, and the
        // reader's result navigator steps through them.
        let mut step = std::mem::take(&mut self.pending_step);
        if self.field_focused {
            ui.input_mut(|i| {
                if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                    step += 1;
                }
                if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                    step -= 1;
                }
                if i.key_pressed(Key::Enter) {
                    jump_to = self.results.get(self.selected).copied();
                }
            });
        }
        if step != 0 && !self.results.is_empty() {
            self.selected =
                (self.selected as i64 + step as i64).clamp(0, self.results.len() as i64 - 1) as usize;
            self.scroll_to_selected = true;
            jump_to = self.results.get(self.selected).copied();
        }

        let book = location.0.book();
        // The results panel's 4 pt top padding plus the controls' own 4.
        egui::Frame::NONE.inner_margin(egui::Margin { left: 8, right: 8, top: 8, bottom: 4 }).show(
            ui,
            |ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                let idx = match self.scope {
                    Scope::OldTestament => 1,
                    Scope::NewTestament => 2,
                    Scope::Book(b) if b >= FIRST_NT_BOOK => 2,
                    Scope::Book(_) => 1,
                    Scope::All => 0,
                };
                if let Some(i) = widgets::segmented(ui, p, &["Bible", "Old Testament", "New Testament"], idx)
                {
                    self.scope = [Scope::All, Scope::OldTestament, Scope::NewTestament][i];
                }
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                let select = Rect::from_min_size(row.min, vec2(168.0, 32.0));
                let label = match self.scope {
                    Scope::Book(b) => b.name().to_string(),
                    _ => "All books".into(),
                };
                let resp = select_field(ui, p, select, &label, "scope-book");
                crate::menu::popup(&resp, p).show(|ui| {
                    ui.set_min_width(200.0);
                    let testament = match self.scope {
                        Scope::OldTestament => Some(false),
                        Scope::NewTestament => Some(true),
                        _ => None,
                    };
                    if ui.selectable_label(!matches!(self.scope, Scope::Book(_)), "All books").clicked() {
                        self.scope = match testament {
                            Some(false) => Scope::OldTestament,
                            Some(true) => Scope::NewTestament,
                            None => Scope::All,
                        };
                        ui.close();
                    }
                    if ui
                        .selectable_label(
                            self.scope == Scope::Book(book),
                            format!("{} (this book)", book.name()),
                        )
                        .clicked()
                    {
                        self.scope = Scope::Book(book);
                        ui.close();
                    }
                    crate::menu::separator(ui);
                    ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                        for b in
                            BookId::all().filter(|b| testament.is_none_or(|nt| (*b >= FIRST_NT_BOOK) == nt))
                        {
                            if ui.selectable_label(self.scope == Scope::Book(b), b.name()).clicked() {
                                self.scope = Scope::Book(b);
                                ui.close();
                            }
                        }
                    });
                });
                let check =
                    Rect::from_min_size(pos2(select.right() + 16.0, row.center().y - 8.0), vec2(16.0, 16.0));
                if widgets::checkbox(ui, p, check, *whole_words, "Whole words").clicked() {
                    *whole_words = !*whole_words;
                }

                // Run the search when its inputs change (well under a millisecond on the KJV).
                let key = (self.query.clone(), translation.to_string(), self.scope, *whole_words);
                if self.key.as_ref() != Some(&key) || self.order_key != (self.current_first, book) {
                    let out = linger_core::search::run(&t, &self.query, self.scope, *whole_words);
                    let mut verses = out.verses;
                    if self.current_first {
                        // Stable: this book's hits first, the rest in canonical order.
                        verses.sort_by_key(|vi| t.verse_view(*vi).is_none_or(|v| v.chapter.book() != book));
                    }
                    self.results = Arc::new(verses);
                    self.elapsed = out.elapsed;
                    self.key = Some(key);
                    self.order_key = (self.current_first, book);
                    self.selected = 0;
                    let query = Query::parse(&self.query, *whole_words);
                    self.entries = Arc::new(result_entries(ui, &t, &self.results, &query));
                    self.suggestion = if self.results.is_empty() {
                        linger_core::search::suggest(&t, &self.query, self.scope, *whole_words)
                    } else {
                        None
                    };
                }
                if self.results.is_empty() {
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
                    ui.painter().text(
                        pos2(row.left() + 4.0, row.top() + 4.0 + 8.0),
                        Align2::LEFT_CENTER,
                        "No verses match",
                        theme::ui_medium(text::SMALL.size),
                        p.ink_2,
                    );
                    return;
                }

                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
                let n = self.results.len();
                let books = self.entries.iter().filter(|e| matches!(e.kind, EntryKind::Header(..))).count();
                let summary = format!(
                    "{} {} in {} {}",
                    group(n),
                    if n == 1 { "verse" } else { "verses" },
                    books,
                    if books == 1 { "book" } else { "books" }
                );
                let font = theme::ui_medium(text::SMALL.size);
                let cy = row.top() + 4.0 + 8.0;
                let r = ui.painter().text(
                    pos2(row.left() + 4.0, cy),
                    Align2::LEFT_CENTER,
                    summary,
                    font.clone(),
                    p.ink_2,
                );
                ui.interact(r, ui.id().with("summary"), Sense::hover()).on_hover_text(format!(
                    "{:.2} ms · {}",
                    self.elapsed.as_secs_f64() * 1000.0,
                    t.meta.abbrev
                ));
                let order = if self.current_first { "This book first" } else { "Canonical" };
                let ow = ui.painter().layout_no_wrap(order.into(), font.clone(), p.ink_2).size().x;
                let chevron = Rect::from_center_size(pos2(row.right() - 4.0 - 6.0, cy), vec2(12.0, 12.0));
                let order_rect = Rect::from_min_max(
                    pos2(chevron.left() - 2.0 - ow, cy - 8.0),
                    pos2(chevron.right(), cy + 8.0),
                );
                let resp = ui.interact(order_rect, ui.id().with("order"), Sense::click());
                ui.painter().text(pos2(chevron.left() - 2.0, cy), Align2::RIGHT_CENTER, order, font, p.ink_2);
                crate::icons::chevron_down(ui.painter(), chevron, p.ink_3);
                crate::menu::popup(&resp, p).show(|ui| {
                    if ui.selectable_label(!self.current_first, "Canonical").clicked() {
                        self.current_first = false;
                        ui.close();
                    }
                    if ui.selectable_label(self.current_first, "This book first").clicked() {
                        self.current_first = true;
                        ui.close();
                    }
                });
            },
        );

        if self.results.is_empty() {
            self.no_results(ui, p, &t, whole_words);
            return;
        }
        let query = Query::parse(&self.query, *whole_words);
        let entries = self.entries.clone();
        let total = entries.last().map_or(0.0, |e| e.y + e.height);
        let mut sa = ScrollArea::vertical().auto_shrink(false).id_salt("results");
        if std::mem::take(&mut self.scroll_to_selected)
            && let Some(e) =
                entries.iter().find(|e| matches!(e.kind, EntryKind::Item(i, _) if i == self.selected))
        {
            let avail = ui.available_height();
            let cur = ui.ctx().data(|d| d.get_temp::<f32>(egui::Id::new("results-offset"))).unwrap_or(0.0);
            if e.y < cur || e.y + e.height > cur + avail {
                sa = sa.vertical_scroll_offset((e.y - avail / 3.0).max(0.0));
            }
        }
        let out = sa.show_viewport(ui, |ui, viewport| {
            let width = ui.available_width();
            ui.set_min_height(total + 8.0);
            let origin = ui.min_rect().min + vec2(LIST_INSET, 0.0);
            let first = entries.partition_point(|e| e.y + e.height < viewport.top());
            for e in &entries[first..] {
                if e.y > viewport.bottom() {
                    break;
                }
                let r =
                    Rect::from_min_size(origin + vec2(0.0, e.y), vec2(width - 2.0 * LIST_INSET, e.height));
                match e.kind {
                    EntryKind::Header(b, count) => {
                        let cy = r.top() + 8.0 + 10.0;
                        let name = t.meta.book_short(b).to_string();
                        ui.painter().text(
                            pos2(r.left() + 8.0, cy),
                            Align2::LEFT_CENTER,
                            name,
                            text::CAPTION.font(),
                            p.ink_3,
                        );
                        ui.painter().text(
                            pos2(r.right() - 8.0, cy),
                            Align2::RIGHT_CENTER,
                            count.to_string(),
                            text::SMALL.font(),
                            p.ink_3,
                        );
                    }
                    EntryKind::Item(i, rows) => {
                        let vi = self.results[i];
                        let Some(v) = t.verse_view(vi) else { continue };
                        let resp = ui.interact(r, ui.id().with(("hit", vi)), Sense::click());
                        let selected = i == self.selected;
                        if selected {
                            ui.painter().rect_filled(r, 10.0, p.accent_soft);
                        } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
                            ui.painter().rect_filled(r, 10.0, fill);
                        }
                        let inner = r.shrink2(ITEM_PAD);
                        let reference = format!(
                            "{} {}:{}",
                            t.meta.ref_name(v.chapter.book()),
                            v.chapter.chapter(),
                            v.number
                        );
                        let cy = inner.top() + 8.0;
                        ui.painter().text(
                            pos2(inner.left(), cy),
                            Align2::LEFT_CENTER,
                            reference,
                            text::UI_STRONG.font(),
                            if selected { p.accent } else { p.ink },
                        );
                        ui.painter().text(
                            pos2(inner.right(), cy),
                            Align2::RIGHT_CENTER,
                            &t.meta.abbrev,
                            theme::ui_medium(11.0),
                            p.ink_3,
                        );
                        let (job, hits) =
                            snippet_job(&v.plain_text(), &query, inner.width(), rows as usize, theme);
                        paint_snippet(ui.painter(), p, pos2(inner.left(), inner.top() + 20.0), job, &hits);
                        if resp.clicked() {
                            self.selected = i;
                            jump_to = Some(vi);
                        }
                    }
                }
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("results-offset"), out.state.offset.y));
        if let Some(vi) = jump_to
            && let Some(v) = t.verse_view(vi)
        {
            actions.push(Action::Jump { chapter: v.chapter, verse: Some(v.number), flash: true });
        }
    }
}

pub(crate) type IconFn = fn(&Painter, Rect, Color32);

fn icon_bookmark(p: &Painter, r: Rect, c: Color32) {
    crate::icons::bookmark(p, r, c, false);
}

/// A source-list row (also Settings' sections).
#[derive(Default)]
pub(crate) struct Row<'a> {
    pub icon: Option<IconFn>,
    /// The icon's colour when not the accent (recent places use tertiary ink).
    pub icon_color: Option<Color32>,
    pub label: &'a str,
    /// A small label at the end (a translation's abbreviation).
    pub trailing: Option<String>,
    /// Extra space before the label (book rows have no icon but line up with icon rows).
    pub indent: f32,
    pub count: Option<String>,
    /// A disclosure chevron: `Some(open)`.
    pub disclosure: Option<bool>,
    pub selected: bool,
}

pub(crate) fn row(ui: &mut Ui, p: &Palette, r: &Row<'_>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    if r.selected {
        ui.painter().rect_filled(rect, theme::radius::SM, p.accent_soft);
    } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
        ui.painter().rect_filled(rect, theme::radius::SM, fill);
    }
    let painter = ui.painter();
    let cy = rect.center().y;
    let mut x = rect.left() + ROW_PAD + r.indent;
    if let Some(icon) = r.icon {
        icon(
            painter,
            Rect::from_min_size(pos2(x, cy - ICON_LANE / 2.0), vec2(ICON_LANE, ICON_LANE)),
            r.icon_color.unwrap_or(p.accent),
        );
        x += ICON_LANE + GAP;
    }
    let (font, color) =
        if r.selected { (text::UI_STRONG.font(), p.accent) } else { (text::UI.font(), p.ink) };
    painter.text(pos2(x, cy), Align2::LEFT_CENTER, r.label, font, color);
    let mut right = rect.right() - ROW_PAD;
    if let Some(open) = r.disclosure {
        let lane =
            Rect::from_min_size(pos2(right - ICON_LANE, cy - ICON_LANE / 2.0), vec2(ICON_LANE, ICON_LANE));
        let chevron = Rect::from_center_size(lane.center(), vec2(12.0, 12.0));
        let color = if r.selected { p.accent } else { p.ink_3 };
        if open {
            crate::icons::chevron_down(painter, chevron, color);
        } else {
            crate::icons::disclosure(painter, chevron, color);
        }
        right = lane.left() - GAP;
    }
    if let Some(count) = &r.count {
        let (font, color) = if r.selected {
            (theme::ui_medium(text::SMALL.size), p.accent)
        } else {
            (text::SMALL.font(), p.ink_3)
        };
        painter.text(pos2(right, cy), Align2::RIGHT_CENTER, count, font, color);
    }
    if let Some(t) = &r.trailing {
        painter.text(pos2(right, cy), Align2::RIGHT_CENTER, t, theme::ui_medium(11.0), p.ink_3);
    }
    resp
}

/// A bookmark, highlight, note or search hit in a list (`ds-09`): reference and translation,
/// then the note (13/18) and a serif snippet (14/20, two lines, or one under a note).
pub(crate) struct ListItem {
    pub reference: String,
    pub trailing: String,
    /// A highlight's 3 pt colour bar.
    pub bar: Option<Color32>,
    pub note: Option<String>,
    pub snippet: String,
    pub selected: bool,
}

const ITEM_PAD: egui::Vec2 = vec2(12.0, 10.0);

fn wrapped(text: &str, font: FontId, line_height: f32, color: Color32, width: f32, rows: usize) -> LayoutJob {
    let mut job = LayoutJob::single_section(
        text.to_string(),
        TextFormat { font_id: font, color, line_height: Some(line_height), ..Default::default() },
    );
    job.wrap = TextWrapping {
        max_width: width,
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    job
}

pub(crate) fn list_item(ui: &mut Ui, p: &Palette, item: &ListItem) -> Response {
    let width = ui.available_width();
    let mut text_left = ITEM_PAD.x;
    if item.bar.is_some() {
        text_left += 3.0 + GAP;
    }
    let text_w = width - text_left - ITEM_PAD.x;
    let painter = ui.painter().clone();
    let note =
        item.note.as_ref().map(|n| painter.layout_job(wrapped(n, text::UI.font(), 18.0, p.ink, text_w, 2)));
    let snippet_rows = if note.is_some() { 1 } else { 2 };
    let snippet = painter.layout_job(wrapped(
        &item.snippet,
        FontId::new(text::SNIPPET_SM.size, theme::family(theme::SERIF)),
        text::SNIPPET_SM.line_height,
        p.ink_2,
        text_w,
        snippet_rows,
    ));
    let mut h = ITEM_PAD.y + 16.0 + 4.0;
    if let Some(n) = &note {
        h += n.size().y + 4.0;
    }
    h += snippet.size().y + ITEM_PAD.y;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    if item.selected {
        painter.rect_filled(rect, 10.0, p.accent_soft);
    } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
        painter.rect_filled(rect, 10.0, fill);
    }
    let inner = rect.shrink2(ITEM_PAD);
    if let Some(c) = item.bar {
        painter.rect_filled(Rect::from_min_size(inner.min, vec2(3.0, inner.height())), 2.0, c);
    }
    let x = rect.left() + text_left;
    let mut y = inner.top();
    painter.text(
        pos2(x, y + 8.0),
        Align2::LEFT_CENTER,
        &item.reference,
        text::UI_STRONG.font(),
        if item.selected { p.accent } else { p.ink },
    );
    painter.text(
        pos2(inner.right(), y + 8.0),
        Align2::RIGHT_CENTER,
        &item.trailing,
        theme::ui_medium(11.0),
        p.ink_3,
    );
    y += 16.0 + 4.0;
    if let Some(n) = note {
        let h = n.size().y;
        painter.galley(pos2(x, y), n, p.ink);
        y += h + 4.0;
    }
    painter.galley(pos2(x, y), snippet, p.ink_2);
    resp
}

/// Highlight colour chips: "All", then one swatch per colour (`ds-02`).
fn colour_filter(ui: &mut Ui, p: &Palette, filter: &mut Option<HighlightColor>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let cy = rect.top() + 4.0 + 10.0;
    let x = rect.left() + 32.0;
    let all_font = theme::ui_semibold(11.0);
    let all_w = ui.painter().layout_no_wrap("All".into(), all_font.clone(), p.ink).size().x + 16.0;
    let all = Rect::from_min_size(pos2(x, cy - 10.0), vec2(all_w, 20.0));
    let resp = ui.interact(all, ui.id().with("hl-all"), Sense::click());
    let on = filter.is_none();
    let fill = if on { p.accent } else { widgets::hover_fill(ui, &resp, p, false).unwrap_or(p.fill) };
    ui.painter().rect_filled(all, 10.0, fill);
    ui.painter().text(
        all.center(),
        Align2::CENTER_CENTER,
        "All",
        all_font,
        if on { p.on_accent } else { p.ink_2 },
    );
    if resp.clicked() {
        *filter = None;
    }
    let mut x = all.right() + GAP;
    for c in HighlightColor::ALL {
        let r = Rect::from_min_size(pos2(x, cy - 8.0), vec2(16.0, 16.0));
        let resp = ui.interact(r.expand(2.0), ui.id().with(("hl", c.name())), Sense::click());
        let selected = *filter == Some(c);
        if selected {
            ui.painter().circle_stroke(r.center(), 8.0 + 2.75, egui::Stroke::new(1.5, p.ink));
        } else if resp.hovered() {
            ui.painter().circle_filled(r.center(), 8.0 + 3.0, p.fill_strong);
        }
        ui.painter().circle_filled(r.center(), 8.0, p.swatch(c));
        ui.painter().circle_stroke(r.center(), 7.75, egui::Stroke::new(0.5, Color32::from_black_alpha(46)));
        if resp.on_hover_text(format!("Only {} highlights", c.name())).clicked() {
            *filter = if selected { None } else { Some(c) };
        }
        x = r.right() + GAP;
    }
}

/// A section header: 11 pt semibold tertiary text, 8 pt below the previous row (16 for later
/// sections).
/// The sidebar footer's height.
pub const FOOTER: f32 = 44.0;

/// What the sidebar footer shows.
pub enum Footer<'a> {
    Status(&'a SyncStatus),
    /// Sync couldn't start.
    Error(&'a str),
    /// The library consent sheet was declined: the library stays on this computer.
    Off,
}

/// The sidebar footer: the Settings gear, then the library's sync status when it syncs. Quiet
/// when synced, the count waiting when offline, the error with Retry when something was refused,
/// and Turn On when sync is off.
pub fn footer(ui: &mut Ui, p: &Palette, rect: Rect, status: Option<Footer<'_>>, actions: &mut Vec<Action>) {
    let painter = ui.painter_at(rect);
    painter.hline(rect.x_range().shrink(LIST_INSET), rect.top() + 0.5, egui::Stroke::new(1.0, p.separator));
    let cy = rect.center().y;
    // The gear's glyph lines up with the rows' icons above it.
    let gear = Rect::from_center_size(
        pos2(rect.left() + LIST_INSET + ROW_PAD + ICON_LANE / 2.0, cy),
        vec2(28.0, 28.0),
    );
    if widgets::round_button(ui, p, gear, crate::icons::gear, ui.id().with("open-settings"))
        .on_hover_text("Settings (⌘,)")
        .clicked()
    {
        actions.push(Action::OpenSettings(Default::default()));
    }

    let waiting = |n: usize| format!("{n} change{} waiting", if n == 1 { "" } else { "s" });
    let (label, color, button) = match status {
        None | Some(Footer::Status(SyncStatus::SignedOut)) => return,
        Some(Footer::Status(SyncStatus::Synced)) => ("Synced".to_string(), p.ink_3, None),
        Some(Footer::Status(SyncStatus::Syncing(0))) => ("Syncing…".to_string(), p.ink_3, None),
        Some(Footer::Status(SyncStatus::Syncing(n))) => (format!("Syncing · {}", waiting(*n)), p.ink_3, None),
        Some(Footer::Status(SyncStatus::Offline(0))) => ("Offline".to_string(), p.ink_3, None),
        Some(Footer::Status(SyncStatus::Offline(n))) => (format!("Offline · {}", waiting(*n)), p.ink_2, None),
        Some(Footer::Status(SyncStatus::Error(e))) => {
            (e.clone(), p.danger, Some(("Retry", Action::RetrySync)))
        }
        Some(Footer::Error(e)) => (e.to_string(), p.danger, Some(("Retry", Action::RetrySync))),
        Some(Footer::Off) => (
            if cfg!(target_os = "macos") { "Only on this Mac" } else { "Only on this computer" }.to_string(),
            p.ink_3,
            Some(("Turn On Sync", Action::TurnOnSync)),
        ),
    };
    let (left, mut right) = (gear.right() + 4.0, rect.right() - LIST_INSET - ROW_PAD);
    // Both laid out the same way, so they share a baseline.
    let line = |text: &str, color, width: f32| {
        painter.layout_job(wrapped(text, text::CAPTION.font(), 16.0, color, width, 1))
    };
    if let Some((button, action)) = button {
        let galley = line(button, p.accent, f32::INFINITY);
        let w = galley.size().x;
        let hit = Rect::from_min_max(pos2(right - w - 4.0, rect.top()), pos2(right + 4.0, rect.bottom()));
        let resp = ui
            .interact(hit, ui.id().with("sync-retry"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        painter.galley(pos2(right - w, cy - galley.size().y / 2.0), galley, p.accent);
        if resp.clicked() {
            actions.push(action);
        }
        right -= w + GAP;
    }
    let galley = line(&label, color, (right - left).max(0.0));
    painter.galley(pos2(left, cy - galley.size().y / 2.0), galley, color);
}

fn section(ui: &mut Ui, p: &Palette, label: &str, first: bool) {
    let (top, h) = if first { (8.0, 28.0) } else { (16.0, 36.0) };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let y = rect.top() + top + (h - top) / 2.0;
    ui.painter().text(
        pos2(rect.left() + ROW_PAD, y),
        Align2::LEFT_CENTER,
        label,
        text::CAPTION.font(),
        p.ink_3,
    );
}

fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A search hit's snippet: Literata 14/20 in secondary ink, starting a few words before the first
/// hit (marked with "…"), the matched phrase in ink at medium weight. Returns the job and the
/// char ranges of the hits within it.
pub fn snippet_job(
    text: &str,
    query: &Query,
    width: f32,
    rows: usize,
    theme: Theme,
) -> (LayoutJob, Vec<std::ops::Range<usize>>) {
    let p = theme.palette();
    let base = TextFormat {
        font_id: FontId::new(text::SNIPPET_SM.size, theme::family(theme::SERIF)),
        color: p.ink_2,
        line_height: Some(text::SNIPPET_SM.line_height),
        ..Default::default()
    };
    let hit = TextFormat {
        font_id: FontId::new(text::SNIPPET_SM.size, theme::family(theme::SERIF_MEDIUM)),
        color: p.ink,
        ..base.clone()
    };
    let ranges = hit_ranges(text, query);
    let start = kwic_start(text, &ranges);
    let mut job = LayoutJob::default();
    let mut chars = 0usize;
    let mut out = Vec::new();
    // A hair of space either side of a hit, for its rounded mark.
    let mut after_hit = false;
    let mut push = |job: &mut LayoutJob, s: &str, f: TextFormat, is_hit: bool| {
        let n = s.chars().count();
        if is_hit {
            out.push(chars..chars + n);
        }
        chars += n;
        job.append(s, if is_hit || after_hit { 2.0 } else { 0.0 }, f);
        after_hit = is_hit;
    };
    if start > 0 {
        push(&mut job, "…", base.clone(), false);
    }
    let mut pos = start;
    for r in ranges.iter().filter(|r| r.start >= start) {
        if r.start > pos {
            push(&mut job, &text[pos..r.start], base.clone(), false);
        }
        push(&mut job, &text[r.start..r.end], hit.clone(), true);
        pos = r.end;
    }
    if pos < text.len() {
        push(&mut job, &text[pos..], base, false);
    }
    job.wrap = TextWrapping {
        max_width: width,
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    job.halign = Align::LEFT;
    (job, out)
}

/// Matches of the query in the text, with hits separated only by spaces merged ("love one another").
fn hit_ranges(text: &str, query: &Query) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for r in query.match_ranges(text) {
        match ranges.last_mut() {
            Some(last) if text[last.end..r.start].chars().all(|c| c == ' ') => last.end = r.end,
            _ => ranges.push(r),
        }
    }
    ranges
}

/// Where a snippet starts: five words before the first hit, or at the verse's start when the hit
/// is within its first seven words.
fn kwic_start(text: &str, ranges: &[std::ops::Range<usize>]) -> usize {
    let Some(hit) = ranges.first() else { return 0 };
    let starts: Vec<usize> = text[..hit.start]
        .char_indices()
        .filter(|&(i, c)| !c.is_whitespace() && (i == 0 || text[..i].ends_with(char::is_whitespace)))
        .map(|(i, _)| i)
        .collect();
    if starts.len() <= 7 { 0 } else { starts[starts.len() - 5] }
}

/// Lays out a snippet and paints it with its hits marked (`hl-yellow`, radius 3).
fn paint_snippet(
    painter: &Painter,
    p: &Palette,
    pos: egui::Pos2,
    job: LayoutJob,
    hits: &[std::ops::Range<usize>],
) {
    let galley = painter.layout_job(job);
    for range in hits {
        let mut base = 0usize;
        for row in &galley.rows {
            let n = row.glyphs.len();
            let (s, e) = (range.start.max(base), range.end.min(base + n));
            if s < e {
                let (g0, g1) = (&row.glyphs[s - base], &row.glyphs[e - base - 1]);
                let r = Rect::from_min_max(
                    pos + vec2(row.pos.x + g0.pos.x - 2.0, row.pos.y),
                    pos + vec2(row.pos.x + g1.pos.x + g1.advance_width + 2.0, row.pos.y + row.height()),
                );
                painter.rect_filled(r, 3.0, p.highlights[0]);
            }
            base += n + usize::from(row.ends_with_newline);
        }
    }
    painter.galley(pos, galley, p.ink_2);
}

/// One line of the result list: a book's header or a hit (its index in the results and how many
/// snippet lines it shows), at a precomputed position so the list can be virtualized.
#[derive(Clone, Copy)]
enum EntryKind {
    Header(BookId, usize),
    Item(usize, u8),
}

#[derive(Clone, Copy)]
struct Entry {
    kind: EntryKind,
    y: f32,
    height: f32,
}

/// Lays the result list out: headers 28 pt, hits 60 (one snippet line) or 80 (two). A snippet's
/// line count comes from its glyph advances (no shaping), for the first couple of thousand hits.
fn result_entries(ui: &Ui, t: &linger_core::Translation, results: &[u32], query: &Query) -> Vec<Entry> {
    let width = ui.available_width() - 2.0 * LIST_INSET - 2.0 * ITEM_PAD.x;
    let font = FontId::new(text::SNIPPET_SM.size, theme::family(theme::SERIF));
    let mut out = Vec::with_capacity(results.len() + 64);
    let mut y = 0.0;
    let mut i = 0;
    while i < results.len() {
        let Some(book) = t.verse_view(results[i]).map(|v| v.chapter.book()) else {
            i += 1;
            continue;
        };
        let count = results[i..]
            .iter()
            .take_while(|vi| t.verse_view(**vi).is_some_and(|v| v.chapter.book() == book))
            .count();
        out.push(Entry { kind: EntryKind::Header(book, count), y, height: 28.0 });
        y += 28.0;
        for (k, vi) in results.iter().enumerate().skip(i).take(count) {
            let rows = if k < 2000 {
                let text = t.verse_view(*vi).map(|v| v.plain_text()).unwrap_or_default();
                let start = kwic_start(&text, &hit_ranges(&text, query));
                let w: f32 = ui.fonts_mut(|f| text[start..].chars().map(|c| f.glyph_width(&font, c)).sum());
                if w + if start > 0 { 12.0 } else { 0.0 } <= width { 1 } else { 2 }
            } else {
                2
            };
            let height = 40.0 + 20.0 * rows as f32;
            out.push(Entry { kind: EntryKind::Item(k, rows), y, height });
            y += height;
        }
        i += count;
    }
    out
}

/// A pop-up button (`ds-14` select): field colour, hairline and soft drop, book glyph, value,
/// up-down chevrons.
fn select_field(ui: &mut Ui, p: &Palette, r: Rect, value: &str, id: &str) -> Response {
    let resp = ui.interact(r, ui.id().with(id), Sense::click());
    let painter = ui.painter();
    painter.add(
        eframe::epaint::RectShape::filled(r.translate(vec2(0.0, 1.0)), 8.0, Color32::from_black_alpha(15))
            .with_blur_width(2.0),
    );
    painter.rect_filled(r, 8.0, if resp.hovered() { p.raised } else { p.field });
    painter.rect_stroke(r, 8.0, egui::Stroke::new(0.5, p.border), egui::StrokeKind::Outside);
    let cy = r.center().y;
    crate::icons::heavy(|| {
        crate::icons::book(
            painter,
            Rect::from_center_size(pos2(r.left() + 12.0 + 8.0, cy), vec2(16.0, 16.0)),
            p.ink_2,
        )
    });
    painter.text(pos2(r.left() + 12.0 + 16.0 + 8.0, cy), Align2::LEFT_CENTER, value, text::UI.font(), p.ink);
    crate::icons::heavy(|| {
        crate::icons::chevrons_up_down(
            painter,
            Rect::from_center_size(pos2(r.right() - 8.0 - 8.0, cy), vec2(16.0, 16.0)),
            p.ink_2,
        )
    });
    resp
}
