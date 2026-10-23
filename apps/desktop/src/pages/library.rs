//! Library: what the library holds on this Mac — sermons, notes and pages, newest first, by
//! kind — and an item's own view: a sermon's summary, outline and transcript with times, a note's
//! text, and the passages each is anchored to. The toolbar's field searches titles and text, or
//! lists the items on a passage when it is a reference ("Romans 8").

use eframe::egui::{self, Align, FontId, Rect, Sense, Ui, pos2, vec2};

use linger_core::records::{Anchor, Item, ItemKind, ItemStatus};
use linger_core::study::Body;
use linger_core::{VerseRef, reference, transcribe};

use super::{Empty, PageEnv, Tone, elide, group_label, text};
use crate::actions::Action;
use crate::icons;
use crate::theme;
use crate::time;
use crate::widgets::{self, Button, IconFn, Kind, css_baseline, paint_on_baseline};

const ROW: f32 = 64.0;
/// The reading measure of an item's text.
const MEASURE: f32 = 600.0;
const DAY: i64 = 86_400;
const KINDS: [&str; 4] = ["All", "Sermons", "Notes", "Pages"];

#[derive(Default)]
pub struct State {
    /// Which of [`KINDS`] the list shows.
    kind: usize,
    open: Option<Open>,
}

/// The item shown on its own, and for a sermon the moment to show first.
struct Open {
    uid: String,
    at_ms: Option<u64>,
    /// The moment has been scrolled to.
    shown: bool,
}

/// The verse a note is that verse's own note on (one user anchor, one verse), if it is.
pub fn own_verse(study: &linger_core::study::StudyStore, item: &Item) -> Option<VerseRef> {
    if item.kind != ItemKind::Note || item.deleted_at.is_some() {
        return None;
    }
    let mut user = item.anchors.iter().filter(|a| a.origin == linger_core::records::AnchorOrigin::User);
    let v = match (user.next(), user.next()) {
        (Some(a), None) if a.from == a.to => a.from.verse_ref()?,
        _ => return None,
    };
    study.note(v).is_some_and(|n| n.uid == item.uid).then_some(v)
}

/// An item's title as the list and chips show it: its own, or what kind of item it is.
pub fn title(item: &Item) -> String {
    let own = [Some(&item.title), item.details.as_ref().and_then(|d| d.title.as_ref())]
        .into_iter()
        .flatten()
        .map(|t| t.trim())
        .find(|t| !t.is_empty());
    if let Some(t) = own {
        return t.to_string();
    }
    match item.kind {
        ItemKind::Note => item
            .text
            .as_deref()
            .and_then(|t| t.lines().map(str::trim).find(|l| !l.is_empty()))
            .map(|l| {
                let l = l.trim_start_matches(['#', '>', '-', '*', ' ']);
                if l.chars().count() > 60 {
                    format!("{}…", l.chars().take(59).collect::<String>().trim_end())
                } else {
                    l.to_string()
                }
            })
            .unwrap_or_else(|| "Note".into()),
        kind => format!("{}, {}", kind_name(kind), time::short_date(captured(item))),
    }
}

pub fn icon(kind: ItemKind) -> IconFn {
    match kind {
        ItemKind::Sermon => icons::mic,
        ItemKind::Note => icons::note,
        ItemKind::Handwritten | ItemKind::Slide | ItemKind::Page => icons::page,
    }
}

fn kind_name(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Sermon => "Sermon",
        ItemKind::Note => "Note",
        ItemKind::Handwritten => "Handwritten page",
        ItemKind::Slide => "Slide",
        ItemKind::Page => "Page",
    }
}

/// When it was made, in Unix seconds: a sermon's start, else when it was captured.
fn captured(item: &Item) -> i64 {
    (item.details.as_ref().map_or(item.captured_at, |d| d.started_at) / 1000) as i64
}

/// "Sermon · Sarah Lee · Grace Church · Oct 3", with the status while it isn't ready.
fn meta(item: &Item) -> String {
    let mut parts = vec![kind_name(item.kind).to_string()];
    if let Some(d) = &item.details {
        parts.extend(
            [&d.speaker, &d.from]
                .into_iter()
                .flatten()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    parts.push(time::short_date(captured(item)));
    let status = match item.status {
        ItemStatus::Ready => None,
        ItemStatus::Recording => Some("Recording"),
        ItemStatus::Uploading => Some("Uploading"),
        ItemStatus::Waiting | ItemStatus::Processing => Some("Transcribing"),
        ItemStatus::Review => Some("To review"),
        ItemStatus::Error => Some("Couldn't be transcribed"),
    };
    parts.extend(status.map(str::to_string));
    parts.join(" · ")
}

/// The passages an item is anchored to, each once, the user's first.
fn passages(item: &Item) -> Vec<(VerseRef, VerseRef)> {
    let mut out: Vec<(VerseRef, VerseRef)> = Vec::new();
    let mut anchors: Vec<&Anchor> = item.anchors.iter().collect();
    anchors.sort_by_key(|a| a.origin != linger_core::records::AnchorOrigin::User);
    for a in anchors {
        if let (Some(from), Some(to)) = (a.from.verse_ref(), a.to.verse_ref())
            && !out.contains(&(from, to))
        {
            out.push((from, to));
        }
    }
    out
}

impl State {
    /// Shows an item on its own (a citation chip, a margin mark).
    pub fn open(&mut self, uid: &str, at_ms: Option<u64>) {
        self.open = Some(Open { uid: uid.into(), at_ms, shown: false });
    }

    pub fn back(&mut self) {
        self.open = None;
    }

    /// The verse whose own note is open, when it is one: Library shows it in the note editor.
    pub fn open_note(&self, study: &linger_core::study::StudyStore) -> Option<VerseRef> {
        let item = study.item(&self.open.as_ref()?.uid).ok()??;
        own_verse(study, &item)
    }

    pub fn show(&mut self, ui: &mut Ui, content: Rect, env: &PageEnv<'_>, actions: &mut Vec<Action>) -> f32 {
        let p = env.settings.theme.palette();
        if let Some(open) = &self.open
            && let Ok(Some(item)) = env.study.item(&open.uid)
        {
            let at_ms = open.at_ms;
            let shown = open.shown;
            let (bottom, back) = self.item(ui, content, env, &item, at_ms, shown, actions);
            if back {
                self.open = None;
            }
            return bottom;
        }
        self.open = None;
        let items = env.study.items().unwrap_or_default();
        let (left, right, top) = (content.left(), content.right(), content.top());
        let painter = ui.painter().clone();
        let subtitle = match items.len() {
            0 => "Nothing here yet".to_string(),
            1 => "1 item, on every device you're signed in on".into(),
            n => format!("{n} items, on every device you're signed in on"),
        };
        super::title(&painter, p, left, top, "Library", &subtitle);
        if items.is_empty() {
            let empty = Empty {
                glyph: icons::folder,
                tone: Tone::Accent,
                title: "Your library is empty",
                body: "Sermons you record, answers you keep from Ask and pages you photograph are kept \
                       here, anchored to the verses they're about, and reach your other devices.",
                buttons: &[],
                hints: &[(&["⌘", "J"], "Ask about what you're reading")],
                width: 380.0,
            };
            return super::bookmarks::empty_page(ui, p, content, &empty, actions);
        }

        let cy = top + 26.0 + 16.0;
        let (_, pick) =
            widgets::segmented_tight(ui, p, pos2(right, cy), ui.id().with("library-kind"), &KINDS, self.kind);
        if let Some(i) = pick {
            self.kind = i;
        }
        let kind = |item: &Item| match self.kind {
            1 => item.kind == ItemKind::Sermon,
            2 => item.kind == ItemKind::Note,
            3 => !matches!(item.kind, ItemKind::Sermon | ItemKind::Note),
            _ => true,
        };
        // A reference lists the items on that passage; anything else searches titles and text.
        let q = env.query.trim();
        let on_passage = reference::parse(q).filter(|_| !q.is_empty()).map(|r| {
            let spans = env.study.spans_in_chapter(r.chapter_idx()).unwrap_or_default();
            let uids: Vec<String> = spans
                .into_iter()
                .filter(|s| r.verse.is_none_or(|v| (s.first..=s.last).contains(&v)))
                .map(|s| s.uid)
                .collect();
            uids
        });
        let words = q.to_lowercase();
        let matches = |item: &Item| match &on_passage {
            Some(uids) => uids.contains(&item.uid),
            None if words.is_empty() => true,
            None => {
                let mut hay = vec![title(item), meta(item)];
                hay.extend([&item.excerpt, &item.summary].into_iter().flatten().cloned());
                match env.study.body(item).ok().flatten() {
                    Some(Body::Plain(t)) => hay.push(t),
                    Some(Body::Transcript(t)) => hay.extend(t.segments.into_iter().map(|s| s.text)),
                    None => {}
                }
                hay.iter().any(|h| h.to_lowercase().contains(&words))
            }
        };
        let shown: Vec<&Item> = items.iter().filter(|i| kind(i) && matches(i)).collect();
        let body = top + 58.0 + 20.0;
        if shown.is_empty() {
            let empty = Empty {
                glyph: icons::search,
                tone: Tone::Quiet,
                title: "Nothing matches",
                body: "Try other words, or a reference such as Romans 8 for what's anchored there.",
                buttons: &[],
                hints: &[],
                width: 360.0,
            };
            let h = empty.height(&painter, content.width());
            let y = super::optical_top(super::under_toolbar(ui, content), h, body);
            empty.show(ui, p, content.center().x, y, content.width());
            return y + h;
        }
        let today = time::day_start(linger_core::study::now());
        let mut y = body;
        let mut day = None;
        for item in shown {
            let made = captured(item);
            let d = time::day_start(made);
            if day != Some(d) {
                if day.is_some() {
                    y += 16.0;
                }
                day = Some(d);
                let label = if d == today {
                    "Today".to_string()
                } else if d >= today - 6 * DAY {
                    time::weekday(time::local(made).weekday).to_string()
                } else {
                    time::short_date(made)
                };
                group_label(&painter, p, left, y, &label);
                y += 16.0 + 8.0;
            }
            let r = Rect::from_min_size(pos2(left, y), vec2(right - left, ROW));
            let resp = ui.interact(r, ui.id().with(("library-item", &item.uid)), Sense::click());
            if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
                painter.rect_filled(r.expand2(vec2(12.0, 0.0)), 12.0, fill);
            }
            painter.circle_filled(pos2(left + 16.0, r.center().y), 16.0, p.fill);
            (icon(item.kind))(
                &painter,
                Rect::from_center_size(pos2(left + 16.0, r.center().y), vec2(16.0, 16.0)),
                p.ink_2,
            );
            let x = left + 48.0;
            let date_w = text(
                &painter,
                right - 4.0,
                y + 14.0,
                16.0,
                &time::short_date(made),
                theme::ui_font(12.0),
                p.ink_3,
                true,
            );
            let font = theme::ui_semibold(13.0);
            let t = elide(&painter, &title(item), &font, right - date_w - 16.0 - x);
            text(&painter, x, y + 12.0, 20.0, &t, font, p.ink, false);
            let m = elide(&painter, &meta(item), &theme::ui_font(12.0), right - x);
            text(&painter, x, y + 34.0, 16.0, &m, theme::ui_font(12.0), p.ink_3, false);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.open(&item.uid, None);
            }
            y += ROW;
        }
        y
    }

    /// An item on its own. Returns the bottom, and whether the way back was taken.
    #[allow(clippy::too_many_arguments)]
    fn item(
        &mut self,
        ui: &mut Ui,
        content: Rect,
        env: &PageEnv<'_>,
        item: &Item,
        at_ms: Option<u64>,
        shown: bool,
        actions: &mut Vec<Action>,
    ) -> (f32, bool) {
        let p = env.settings.theme.palette();
        let painter = ui.painter().clone();
        let (left, top) = (content.left(), content.top());
        let width = content.width().min(MEASURE);
        let (_, back) = Button::new(Kind::Plain, "Library").small().icon(icons::chevron_left).show_left(
            ui,
            p,
            left - 10.0,
            top + 12.0,
            ui.id().with("library-back"),
        );
        let mut y = top + 36.0;
        super::title(&painter, p, left, y, &title(item), &meta(item));
        y += 58.0 + 24.0;
        let serif = FontId::new(15.0, theme::family(theme::SERIF));
        let paragraphs = |y: &mut f32, s: &str| {
            for para in s.split("\n\n").map(str::trim).filter(|s| !s.is_empty()) {
                let g = painter.layout(para.to_string(), serif.clone(), p.ink, width);
                let h = g.size().y;
                painter.galley(pos2(left, *y), g, p.ink);
                *y += h + 12.0;
            }
        };
        if item.status == ItemStatus::Error {
            let g = painter.layout(
                "This recording couldn't be transcribed.".into(),
                theme::ui_font(13.0),
                p.danger,
                width,
            );
            let h = g.size().y;
            painter.galley(pos2(left, y), g, p.danger);
            y += h + 24.0;
        }
        if let Some(s) = item.summary.as_deref().filter(|s| !s.trim().is_empty()) {
            group_label(&painter, p, left, y, "Summary");
            y += 24.0;
            paragraphs(&mut y, s);
            y += 12.0;
        }
        if let Some(o) = item.outline.as_deref().filter(|s| !s.trim().is_empty()) {
            group_label(&painter, p, left, y, "Outline");
            y += 24.0;
            for line in o.lines().map(str::trim).filter(|l| !l.is_empty()) {
                let g = painter.layout(line.to_string(), theme::ui_font(13.0), p.ink, width);
                let h = g.size().y.max(20.0);
                painter.galley(pos2(left, y), g, p.ink);
                y += h + 4.0;
            }
            y += 20.0;
        }
        let passages = passages(item);
        if !passages.is_empty() {
            group_label(&painter, p, left, y, "Passages");
            y += 24.0;
            let font = theme::ui_medium(13.0);
            let mut x = left;
            for (i, (from, to)) in passages.iter().enumerate() {
                let label = reference::format_range(*from, *to);
                let g = painter.layout_no_wrap(label, font.clone(), p.accent);
                let w = g.size().x + 20.0;
                if x > left && x + w > content.right() {
                    x = left;
                    y += 28.0 + 6.0;
                }
                let r = Rect::from_min_size(pos2(x, y), vec2(w, 28.0));
                let resp = ui.interact(r, ui.id().with(("library-passage", i)), Sense::click());
                let fill = if resp.hovered() { p.accent.gamma_multiply(0.2) } else { p.accent_soft };
                painter.rect_filled(r, 14.0, fill);
                painter.galley(pos2(x + 10.0, r.center().y - g.size().y / 2.0), g, p.accent);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    actions.push(Action::Jump {
                        chapter: from.chapter_idx(),
                        verse: Some(from.verse),
                        flash: true,
                    });
                    actions.push(Action::OpenPage(super::Page::Reader));
                }
                x += w + 6.0;
            }
            y += 28.0 + 24.0;
        }
        match env.study.body(item).ok().flatten() {
            Some(Body::Plain(t)) => {
                group_label(&painter, p, left, y, "Text");
                y += 24.0;
                paragraphs(&mut y, &t);
            }
            Some(Body::Transcript(t)) => {
                group_label(&painter, p, left, y, "Transcript");
                y += 24.0;
                let gutter = 56.0;
                for s in &t.segments {
                    let g = painter.layout(s.text.clone(), serif.clone(), p.ink, width - gutter);
                    let h = g.size().y;
                    let row = Rect::from_min_size(pos2(left, y), vec2(width, h));
                    if at_ms.is_some_and(|ms| (s.start_ms..s.end_ms.max(s.start_ms + 1)).contains(&ms)) {
                        painter.rect_filled(row.expand2(vec2(8.0, 6.0)), 8.0, p.accent_soft);
                        if !shown {
                            ui.scroll_to_rect(row.expand2(vec2(0.0, 120.0)), Some(Align::Center));
                            if let Some(o) = &mut self.open {
                                o.shown = true;
                            }
                        }
                    }
                    let clock =
                        painter.layout_no_wrap(transcribe::clock(s.start_ms), theme::ui_font(12.0), p.ink_3);
                    paint_on_baseline(&painter, left, css_baseline(y, 24.0, 12.0, false), clock, p.ink_3);
                    painter.galley(pos2(left + gutter, y), g, p.ink);
                    y += h + 12.0;
                }
            }
            None if item.kind == ItemKind::Sermon && item.status == ItemStatus::Ready => {
                let g = painter.layout(
                    "The transcript hasn't reached this Mac yet.".into(),
                    theme::ui_font(13.0),
                    p.ink_3,
                    width,
                );
                let h = g.size().y;
                painter.galley(pos2(left, y), g, p.ink_3);
                y += h;
            }
            None => {}
        }
        (y, back.clicked())
    }
}
