//! The book picker under the toolbar's location button: one scrolling list of the books under
//! their testament headers, with the open book's chapters in a grid beneath its row (an
//! accordion), and a filter at the top. Rows and cells are menu cells in the menu frame (the
//! `menu` tokens), the book and chapter being read selected.
//!
//! It opens on the book being read, expanded and scrolled into view. Typing filters the books the
//! way the palette reads references ("1 co", and "ps 23" lands on the chapter); backspace
//! deletes. The keys go to the picker rather than to a focused field, so ⌘C still copies the
//! reader's selection. Up and down move one highlight through the books and the open book's
//! chapter rows, left and right along the chapters and in and out of a book; return opens a book
//! or chooses a chapter and escape closes.

use eframe::egui::{
    self, Align, Align2, Event, Id, Key, Modifiers, Rect, Response, ScrollArea, Sense, Stroke, StrokeKind,
    Ui, Vec2, pos2, vec2,
};
use linger_core::books::{BookId, ChapterIdx, FIRST_NT_BOOK};
use linger_core::model::TranslationMeta;
use linger_core::reference;

use crate::actions::Action;
use crate::menu::{self, Cell, ITEM_HEIGHT, PADDING_X, PADDING_Y, SEPARATOR_HEIGHT};
use crate::theme::{Palette, icon, menu as m, text};

const POPUP: &str = "book-picker";
/// The picker's width, frame included.
pub const WIDTH: f32 = 320.0;
/// The tallest the list grows; a short window shortens it.
pub const LIST_HEIGHT: f32 = 460.0;
const CHAPTER_COLUMNS: usize = 7;
const FIELD_HEIGHT: f32 = 32.0;
const FIELD_RADIUS: f32 = 8.0;
/// Between the filter and the list.
const FIELD_GAP: f32 = 6.0;
/// Below an open book's chapters, before the next book.
const CHAPTERS_GAP: f32 = 4.0;
/// Room the picker leaves below it in a short window.
const WINDOW_MARGIN: f32 = 8.0;
const BOOKS: usize = 66;

/// The picker's filter, open book and highlight, while it's open.
#[derive(Clone, Default)]
struct State {
    query: String,
    /// The book whose chapters show.
    open: Option<BookId>,
    cursor: Option<Spot>,
    /// Whether it has opened on the book being read yet.
    started: bool,
    /// Scroll the list to the book being read next frame (on opening and clearing the filter).
    place: bool,
    /// Scroll the open book's chapters into view next frame (it was just opened).
    reveal: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Spot {
    Book(BookId),
    /// A chapter (from 0) of the open book.
    Chapter(usize),
}

/// Keys the picker reads while open.
#[derive(Default)]
struct Keys {
    dx: isize,
    dy: isize,
    enter: bool,
    /// Typed text, and backspaces, for the filter.
    typed: String,
    backspace: bool,
}

/// What a row of the list is.
enum Entry {
    /// A testament header, with a separator above it after the other testament.
    Header(&'static str, bool),
    Book(BookId),
    Chapters(BookId),
    /// Nothing matches the filter.
    Empty,
}

/// Whether the picker is open (it takes the arrow keys, return and escape then).
pub fn is_open(ctx: &egui::Context) -> bool {
    egui::Popup::is_id_open(ctx, Id::new(POPUP))
}

/// The picker, opening from `anchor`; `here` is the chapter being read.
pub fn show(
    anchor: &Response,
    p: &Palette,
    meta: Option<&TranslationMeta>,
    here: ChapterIdx,
    actions: &mut Vec<Action>,
) {
    let state_id = Id::new(POPUP).with("state");
    // The list as tall as the window allows below the anchor, up to its full height.
    let below = anchor.ctx.content_rect().bottom() - anchor.rect.bottom();
    let list_height = (below - 2.0 * PADDING_Y - FIELD_HEIGHT - FIELD_GAP - 2.0 * WINDOW_MARGIN)
        .clamp(4.0 * ITEM_HEIGHT, LIST_HEIGHT);
    let shown = menu::popup(anchor, p)
        .id(Id::new(POPUP))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let mut state: State = ui.data(|d| d.get_temp(state_id)).unwrap_or_default();
            if !state.started {
                state = State { open: Some(here.book()), started: true, place: true, ..State::default() };
            }
            let keys = ui.input_mut(|i| {
                let mut typed = String::new();
                i.events.retain(|e| match e {
                    Event::Text(t) => {
                        typed.push_str(t);
                        false
                    }
                    _ => true,
                });
                Keys {
                    dx: i.consume_key(Modifiers::NONE, Key::ArrowRight) as isize
                        - i.consume_key(Modifiers::NONE, Key::ArrowLeft) as isize,
                    dy: i.consume_key(Modifiers::NONE, Key::ArrowDown) as isize
                        - i.consume_key(Modifiers::NONE, Key::ArrowUp) as isize,
                    enter: i.consume_key(Modifiers::NONE, Key::Enter),
                    backspace: i.consume_key(Modifiers::NONE, Key::Backspace),
                    typed,
                }
            });
            picker(ui, p, meta, here, list_height, &keys, &mut state, actions);
            ui.data_mut(|d| d.insert_temp(state_id, state));
        });
    // Closed: it opens on the book being read again.
    if shown.is_none() {
        anchor.ctx.data_mut(|d| d.remove::<State>(state_id));
    }
}

fn book_name(meta: Option<&TranslationMeta>, b: BookId) -> &str {
    meta.map_or(b.name(), |m| m.book_short(b))
}

/// The books the filter shows, in canonical order, and the best match with the chapter typed,
/// if one was.
fn matches(query: &str) -> (Vec<BookId>, Option<(BookId, Option<usize>)>) {
    let query = query.trim();
    if query.is_empty() {
        return (BookId::all().collect(), None);
    }
    let found = reference::suggest(query, BOOKS);
    let mut books: Vec<BookId> = found.iter().map(|r| r.book).collect();
    books.sort();
    books.dedup();
    // "ps 23" names a chapter; "1 co" doesn't.
    let chapter_typed = query.contains(char::is_alphabetic) && query.ends_with(|c: char| c.is_ascii_digit());
    let best = found
        .first()
        .map(|r| (r.book, (chapter_typed && r.book.chapters() > 1).then(|| r.chapter as usize - 1)));
    (books, best)
}

/// The rows of the list: each testament's books under its header, the open one's chapters after
/// it.
fn entries(books: &[BookId], open: Option<BookId>) -> Vec<Entry> {
    let mut out = Vec::new();
    for (title, nt) in [("Old Testament", false), ("New Testament", true)] {
        let mut group = books.iter().filter(|b| (**b >= FIRST_NT_BOOK) == nt).peekable();
        if group.peek().is_none() {
            continue;
        }
        out.push(Entry::Header(title, !out.is_empty()));
        for &b in group {
            out.push(Entry::Book(b));
            if open == Some(b) && b.chapters() > 1 {
                out.push(Entry::Chapters(b));
            }
        }
    }
    if out.is_empty() {
        out.push(Entry::Empty);
    }
    out
}

fn chapter_rows(b: BookId) -> usize {
    (b.chapters() as usize).div_ceil(CHAPTER_COLUMNS)
}

fn entry_height(e: &Entry) -> f32 {
    match e {
        Entry::Header(_, sep) => m::HEADER_HEIGHT + if *sep { SEPARATOR_HEIGHT } else { 0.0 },
        Entry::Book(_) | Entry::Empty => ITEM_HEIGHT,
        Entry::Chapters(b) => chapter_rows(*b) as f32 * ITEM_HEIGHT + CHAPTERS_GAP,
    }
}

/// Where the arrow keys take the highlight from `from`, and which book is open after: up and down
/// a row at a time through the books and the open book's chapters, left and right along the
/// chapters, and right into a book (opening it, then its first chapter) and left back out.
fn step(from: Spot, open: Option<BookId>, books: &[BookId], keys: &Keys) -> (Spot, Option<BookId>) {
    let pos = |b: BookId| books.iter().position(|x| *x == b);
    let neighbour = |b: BookId, d: isize| {
        pos(b).and_then(|i| i.checked_add_signed(d)).and_then(|i| books.get(i)).copied()
    };
    let opens = |b: BookId| b.chapters() > 1;
    match from {
        Spot::Book(b) if keys.dy > 0 => match open {
            Some(o) if o == b && opens(b) => (Spot::Chapter(0), open),
            _ => (neighbour(b, 1).map_or(from, Spot::Book), open),
        },
        Spot::Book(b) if keys.dy < 0 => match neighbour(b, -1) {
            // Up into the open book above lands on its last row.
            Some(prev) if open == Some(prev) && opens(prev) => {
                (Spot::Chapter((chapter_rows(prev) - 1) * CHAPTER_COLUMNS), open)
            }
            Some(prev) => (Spot::Book(prev), open),
            None => (from, open),
        },
        Spot::Book(b) if keys.dx > 0 && opens(b) => {
            if open == Some(b) {
                (Spot::Chapter(0), open)
            } else {
                (from, Some(b))
            }
        }
        Spot::Book(b) if keys.dx < 0 && open == Some(b) => (from, None),
        Spot::Book(_) => (from, open),
        Spot::Chapter(c) => {
            let Some(b) = open else { return (from, open) };
            let n = b.chapters() as usize;
            let row = c / CHAPTER_COLUMNS;
            if keys.dy > 0 {
                if c + CHAPTER_COLUMNS < n {
                    (Spot::Chapter(c + CHAPTER_COLUMNS), open)
                } else if row + 1 < chapter_rows(b) {
                    // Down from above a short last row: its last chapter.
                    (Spot::Chapter(n - 1), open)
                } else {
                    (neighbour(b, 1).map_or(from, Spot::Book), open)
                }
            } else if keys.dy < 0 {
                (if row == 0 { Spot::Book(b) } else { Spot::Chapter(c - CHAPTER_COLUMNS) }, open)
            } else {
                match c.checked_add_signed(keys.dx) {
                    Some(j) if j < n => (Spot::Chapter(j), open),
                    None => (Spot::Book(b), open),
                    _ => (from, open),
                }
            }
        }
    }
}

fn cell_response(ui: &Ui, r: Rect, id: Id, label: &str, selected: bool) -> Response {
    let resp = ui.interact(r, id, Sense::CLICK);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label));
    resp
}

/// The filter: a field that always has the keyboard, the query or a hint after a magnifier, and
/// a clear button once something is typed. Returns whether the clear button was clicked.
fn filter(ui: &mut Ui, p: &Palette, query: &str) -> bool {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), FIELD_HEIGHT), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(r.expand(3.0), FIELD_RADIUS + 3.0, p.focus);
    painter.rect_filled(r, FIELD_RADIUS, p.field);
    painter.rect_stroke(r, FIELD_RADIUS, Stroke::new(1.0, p.accent), StrokeKind::Inside);
    let x = r.left() + m::ITEM_PADDING;
    let glass = Rect::from_min_size(pos2(x, r.center().y - icon::MENU / 2.0), Vec2::splat(icon::MENU));
    crate::icons::search(painter, glass, p.ink_3);
    let at = pos2(glass.right() + m::ICON_GAP, r.center().y);
    if query.is_empty() {
        painter.text(at, Align2::LEFT_CENTER, "Type to filter books", text::UI.font(), p.ink_3);
        return false;
    }
    let shown = painter.text(at, Align2::LEFT_CENTER, query, text::UI.font(), p.ink);
    // The caret after the text.
    painter.vline(shown.right() + 1.0, shown.y_range().expand(1.0), Stroke::new(1.0, p.accent));
    let c = Rect::from_center_size(pos2(r.right() - m::ITEM_PADDING - 8.0, r.center().y), Vec2::splat(16.0));
    crate::icons::clear(painter, c, p.ink_3, p.field);
    ui.interact(c.expand(2.0), ui.id().with("clear"), Sense::click()).on_hover_text("Clear").clicked()
}

#[allow(clippy::too_many_arguments)]
fn picker(
    ui: &mut Ui,
    p: &Palette,
    meta: Option<&TranslationMeta>,
    here: ChapterIdx,
    list_height: f32,
    keys: &Keys,
    state: &mut State,
    actions: &mut Vec<Action>,
) {
    let width = WIDTH - 2.0 * PADDING_X;
    ui.set_width(width);
    ui.spacing_mut().item_spacing = Vec2::ZERO;

    // The filter: typing narrows the books to the matches and highlights the best one, opening
    // it (on the chapter, if one was typed); clearing it returns to the book being read.
    let before = state.query.clone();
    state.query.extend(keys.typed.chars().filter(|c| !c.is_control()));
    if keys.backspace {
        state.query.pop();
    }
    if filter(ui, p, &state.query) {
        state.query.clear();
    }
    let (books, best) = matches(&state.query);
    let mut scroll_top = false;
    if state.query != before {
        if state.query.trim().is_empty() {
            state.open = Some(here.book());
            state.cursor = None;
            state.place = true;
        } else if let Some((b, chapter)) = best {
            state.open = Some(b);
            state.cursor = Some(chapter.map_or(Spot::Book(b), Spot::Chapter));
            scroll_top = true;
        } else {
            state.cursor = None;
        }
    }
    ui.add_space(FIELD_GAP);

    // The keys move from the highlight, or from where the reader is.
    let current_book = here.book();
    let current_chapter = here.chapter() as usize - 1;
    let moved = keys.dx != 0 || keys.dy != 0;
    if moved {
        let from =
            state.cursor.unwrap_or(if state.open == Some(current_book) && current_book.chapters() > 1 {
                Spot::Chapter(current_chapter)
            } else if books.contains(&current_book) {
                Spot::Book(current_book)
            } else {
                books.first().map_or(Spot::Book(current_book), |b| Spot::Book(*b))
            });
        let (to, open) = step(from, state.open, &books, keys);
        state.cursor = Some(to);
        if open != state.open {
            state.open = open;
            state.reveal = open.is_some();
        }
    }

    let list = entries(&books, state.open);
    // The popup's own room would cap the list short of its height; a short filtered list shrinks.
    let mut scroll = ScrollArea::vertical()
        .id_salt("book-list")
        .max_height(list_height)
        .min_scrolled_height(list_height)
        .auto_shrink([false, true]);
    if scroll_top {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    let mut chosen: Option<ChapterIdx> = None;
    let mut toggled: Option<BookId> = None;
    scroll.show(ui, |ui| {
        let total: f32 = list.iter().map(entry_height).sum();
        let (area, _) = ui.allocate_exact_size(vec2(width, total), Sense::hover());
        let painter = ui.painter().clone();
        let mut y = area.top();
        let mut hovered = None;
        let mut cells: Vec<(Spot, Rect, Response, String)> = Vec::new();
        let mut open_rect: Option<Rect> = None;
        for e in &list {
            let h = entry_height(e);
            let r = Rect::from_min_size(pos2(area.left(), y), vec2(width, h));
            y += h;
            match e {
                Entry::Header(title, sep) => {
                    let mut top = r.top();
                    if *sep {
                        menu::paint_separator(&painter, p, r.left(), r.right(), top);
                        top += SEPARATOR_HEIGHT;
                    }
                    let hr = Rect::from_min_size(pos2(r.left(), top), vec2(width, m::HEADER_HEIGHT));
                    menu::paint_header(&painter, p, hr, title);
                }
                Entry::Empty => {
                    painter.text(
                        pos2(menu::text_left(r), r.center().y),
                        Align2::LEFT_CENTER,
                        "No books match",
                        text::UI.font(),
                        p.ink_3,
                    );
                }
                Entry::Book(b) => {
                    let name = book_name(meta, *b).to_string();
                    let resp = cell_response(ui, r, ui.id().with(("book", b.0)), &name, *b == current_book);
                    if state.open == Some(*b) {
                        open_rect = Some(r);
                    }
                    cells.push((Spot::Book(*b), r, resp, name));
                }
                Entry::Chapters(b) => {
                    if let Some(o) = open_rect.as_mut() {
                        *o = o.union(r);
                    }
                    let cell = vec2(width / CHAPTER_COLUMNS as f32, ITEM_HEIGHT);
                    for i in 0..b.chapters() as usize {
                        let min = r.min
                            + vec2(
                                (i % CHAPTER_COLUMNS) as f32 * cell.x,
                                (i / CHAPTER_COLUMNS) as f32 * cell.y,
                            );
                        let cr = Rect::from_min_size(min, cell);
                        let label = (i + 1).to_string();
                        let selected = *b == current_book && i == current_chapter;
                        let resp = cell_response(ui, cr, ui.id().with(("chapter", b.0, i)), &label, selected);
                        cells.push((Spot::Chapter(i), cr, resp, label));
                    }
                }
            }
        }
        if let Some((spot, ..)) = cells.iter().find(|(_, _, resp, _)| resp.hovered()) {
            hovered = Some(*spot);
        }
        // The pointer and the keyboard share one highlight: it follows the pointer when it moves.
        if ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
            state.cursor = hovered;
        }
        for (spot, r, resp, label) in &cells {
            let (selected, centred) = match spot {
                Spot::Book(b) => (*b == current_book, false),
                Spot::Chapter(i) => (state.open == Some(current_book) && *i == current_chapter, true),
            };
            menu::paint_cell(
                &painter,
                p,
                *r,
                Cell {
                    icon: None,
                    label,
                    centred,
                    selected,
                    lit: state.cursor == Some(*spot),
                    pressed: resp.is_pointer_button_down_on(),
                },
            );
            if let Spot::Book(b) = spot
                && state.open == Some(*b)
                && b.chapters() > 1
            {
                let c = Rect::from_center_size(
                    pos2(r.right() - m::ITEM_PADDING - icon::MENU / 2.0, r.center().y),
                    Vec2::splat(icon::MENU),
                );
                crate::icons::chevron_down(&painter, c, p.ink_3);
            }
            if resp.clicked() {
                match spot {
                    Spot::Book(b) if b.chapters() == 1 => chosen = Some(b.first_chapter()),
                    Spot::Book(b) => toggled = Some(*b),
                    Spot::Chapter(i) => chosen = state.open.and_then(|b| ChapterIdx::new(b, *i as u8 + 1)),
                }
            }
        }

        // Keep the highlight in view, and open on the book being read with a couple of books
        // above it.
        if moved && let Some((_, r, ..)) = cells.iter().find(|(s, ..)| state.cursor == Some(*s)) {
            ui.scroll_to_rect(*r, None);
        }
        if state.place {
            state.place = false;
            if let Some(o) = open_rect {
                ui.scroll_to_rect(
                    Rect::from_min_max(pos2(o.left(), o.top() - 2.0 * ITEM_HEIGHT), o.max),
                    Some(Align::Min),
                );
            }
        }
        // A book just opened brings its chapters into view, as far as they fit.
        if std::mem::take(&mut state.reveal)
            && let Some(o) = open_rect
        {
            ui.scroll_to_rect(Rect::from_min_size(o.min, vec2(o.width(), o.height().min(list_height))), None);
        }
    });

    if keys.enter {
        match state.cursor {
            Some(Spot::Book(b)) if b.chapters() == 1 => chosen = Some(b.first_chapter()),
            Some(Spot::Book(b)) => toggled = Some(b),
            Some(Spot::Chapter(i)) => chosen = state.open.and_then(|b| ChapterIdx::new(b, i as u8 + 1)),
            None => {}
        }
    }
    if let Some(b) = toggled {
        state.open = if state.open == Some(b) { None } else { Some(b) };
        state.reveal = state.open.is_some();
    }
    if let Some(chapter) = chosen {
        actions.push(Action::Jump { chapter, verse: None, flash: false });
        ui.close();
    }
}
