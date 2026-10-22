//! The book picker: one scrolling list of books that opens on the book being read with its
//! chapters beneath it, keeps the menu insets with rows edge to edge, filters as you type and lets
//! the keyboard alone choose a chapter.

use eframe::egui::{self, Key};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use linger::actions::Action;
use linger::book_picker;
use linger::menu::{ITEM_HEIGHT, PADDING_X};
use linger::theme::{self, Theme};
use linger_core::books::{BookId, ChapterIdx};

const JOHN: BookId = BookId(42);
const LUKE: BookId = BookId(41);
const PSALMS: BookId = BookId::PSALMS;
const JUDE: BookId = BookId(64);

/// A button standing in for the toolbar's location button, opening the picker on John 1, with the
/// picker open.
fn open() -> Harness<'static, Vec<Action>> {
    open_in(egui::vec2(1280.0, 800.0))
}

/// The same in a window of `size`.
fn open_in(size: egui::Vec2) -> Harness<'static, Vec<Action>> {
    let here = ChapterIdx::new(JOHN, 1).unwrap();
    let mut harness = Harness::builder().with_size(size).with_pixels_per_point(2.0).build_ui_state(
        move |ui, actions: &mut Vec<Action>| {
            let anchor = ui.button("John 1");
            book_picker::show(&anchor, Theme::LIGHT.palette(), None, here, actions);
        },
        Vec::new(),
    );
    harness.ctx.set_fonts(theme::font_definitions());
    theme::apply(&harness.ctx, Theme::LIGHT);
    harness.run();
    harness.get_by_label("John 1").click();
    harness.run();
    assert!(book_picker::is_open(&harness.ctx));
    harness
}

fn area(harness: &Harness<'_, Vec<Action>>) -> egui::Rect {
    harness.ctx.memory(|m| m.area_rect(egui::Id::new("book-picker"))).expect("the picker is open")
}

fn press(harness: &mut Harness<'_, Vec<Action>>, keys: &[Key]) {
    for key in keys {
        harness.key_press(*key);
        harness.run();
    }
}

fn type_text(harness: &mut Harness<'_, Vec<Action>>, text: &str) {
    harness.event(egui::Event::Text(text.into()));
    harness.run();
}

fn jumped_to(harness: &Harness<'_, Vec<Action>>, book: BookId, chapter: u8) {
    let chapter = ChapterIdx::new(book, chapter).unwrap();
    assert_eq!(*harness.state(), vec![Action::Jump { chapter, verse: None, flash: false }]);
    assert!(!book_picker::is_open(&harness.ctx), "choosing a chapter closes the picker");
}

fn near(a: f32, b: f32, what: &str) {
    assert!((a - b).abs() < 0.01, "{what}: {a} ≠ {b}");
}

/// Whether a cell shows as selected (egui reports a selected button as toggled on).
fn selected(harness: &Harness<'_, Vec<Action>>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

#[test]
fn it_opens_on_the_book_being_read_with_its_chapters_beneath() {
    let harness = open();
    let area = area(&harness);
    let edge = area.shrink(Theme::LIGHT.palette().shadows.popover.ring_width);
    let rect = |label: &str| harness.get_by_label(label).rect();
    let (luke, john, acts) = (rect("Luke"), rect("John"), rect("Acts"));
    let (one, two, eight, twenty_one) = (rect("1"), rect("2"), rect("8"), rect("21"));

    near(edge.width(), book_picker::WIDTH, "picker width");
    let list = area.bottom() - harness.get_by_label("Mark").rect().top();
    assert!(list > book_picker::LIST_HEIGHT - ITEM_HEIGHT, "the list at its full height: {list}");
    // John is scrolled into view, its rows the menu's, inset like menu rows.
    assert!(john.top() >= area.top() && john.bottom() <= area.bottom(), "John in view: {john:?} in {area:?}");
    near(john.left() - edge.left(), PADDING_X, "left inset");
    near(edge.right() - john.right(), PADDING_X, "right inset");
    near(john.height(), ITEM_HEIGHT, "row height");
    near(john.top(), luke.bottom(), "rows abut");
    // Its 21 chapters, seven to a row, sit between John and Acts.
    near(one.top(), john.bottom(), "chapters under the book");
    near(one.left(), john.left(), "chapters from the row's edge");
    near(two.left(), one.right(), "chapters abut");
    near(one.width(), john.width() / 7.0, "seven to a row");
    near(eight.top(), one.bottom(), "the second row");
    near(eight.left(), one.left(), "chapter columns aligned");
    assert!(acts.top() > twenty_one.bottom(), "Acts follows the chapters");
    assert!(harness.query_by_label("22").is_none());
    // Only the open book shows chapters.
    assert!(selected(&harness, "John") && selected(&harness, "1"), "John 1 is selected");
    assert!(!selected(&harness, "Luke") && !selected(&harness, "2"));
}

#[test]
fn the_list_shortens_to_fit_a_short_window() {
    let harness = open_in(egui::vec2(800.0, 420.0));
    let area = area(&harness);
    assert!(area.bottom() <= 420.0, "the picker fits the window: {area:?}");
    let john = harness.get_by_label("John").rect();
    assert!(john.top() >= area.top() && john.bottom() <= area.bottom(), "John in view: {john:?} in {area:?}");
}

#[test]
fn the_keyboard_moves_through_books_and_chapters() {
    let mut harness = open();
    // From John 1: down a row to 8, right to 9, return.
    press(&mut harness, &[Key::ArrowDown, Key::ArrowRight, Key::Enter]);
    jumped_to(&harness, JOHN, 9);

    let mut harness = open();
    // Up out of the chapters to John, up to Luke, right opens it (closing John), right into its
    // first chapter, down a row to 8.
    press(&mut harness, &[Key::ArrowUp, Key::ArrowUp, Key::ArrowRight]);
    assert!(harness.query_by_label("24").is_some(), "Luke's chapters show");
    assert_eq!(harness.query_all_by_label("1").count(), 1, "John's chapters closed");
    assert!(!selected(&harness, "1"), "the chapter 1 showing is Luke's");
    press(&mut harness, &[Key::ArrowRight, Key::ArrowDown, Key::Enter]);
    jumped_to(&harness, LUKE, 8);
}

#[test]
fn typing_filters_the_books() {
    let mut harness = open();
    type_text(&mut harness, "rev");
    assert!(harness.query_by_label("Revelation").is_some());
    assert!(harness.query_by_label("Genesis").is_none() && harness.query_by_label("John").is_none());
    // Backspace deletes; with the filter empty again it's back on John.
    press(&mut harness, &[Key::Backspace, Key::Backspace, Key::Backspace]);
    assert!(harness.query_by_label("Genesis").is_some() && harness.query_by_label("21").is_some());

    // A reference lands on its chapter.
    type_text(&mut harness, "ps 23");
    press(&mut harness, &[Key::Enter]);
    jumped_to(&harness, PSALMS, 23);

    // A book of one chapter goes straight there.
    let mut harness = open();
    type_text(&mut harness, "jude");
    press(&mut harness, &[Key::Enter]);
    jumped_to(&harness, JUDE, 1);
}

#[test]
fn clicking_a_book_opens_and_closes_it_and_escape_closes() {
    let mut harness = open();
    harness.get_by_label("John").click();
    harness.run();
    assert!(harness.query_by_label("21").is_none(), "clicking the open book closes it");
    harness.get_by_label("Luke").click();
    harness.run();
    assert!(harness.query_by_label("24").is_some(), "clicking a book opens it");
    assert!(harness.state().is_empty(), "a book with chapters opens rather than jumping");
    harness.get_by_label("24").click();
    harness.run();
    jumped_to(&harness, LUKE, 24);

    let mut harness = open();
    type_text(&mut harness, "gen");
    harness.key_press(Key::Escape);
    harness.run();
    assert!(!book_picker::is_open(&harness.ctx));
    // It opens on the book being read again, unfiltered.
    harness.get_by_label("John 1").click();
    harness.run();
    assert!(harness.query_by_label("Genesis").is_some() && harness.query_by_label("21").is_some());
}
