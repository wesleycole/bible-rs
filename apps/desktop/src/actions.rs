//! Everything the UI can ask the app to do. Widgets push actions; the app applies them once per
//! frame, so keyboard shortcuts, menus, the palette and buttons all share one code path.

use linger_core::study::HighlightColor;
use linger_core::{ChapterIdx, VerseRef};
use serde::{Deserialize, Serialize};

use crate::theme::{Scheme, ThemeName};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Go somewhere, recording the current position in history.
    Jump {
        chapter: ChapterIdx,
        verse: Option<u8>,
        flash: bool,
    },
    Back,
    Forward,
    PrevChapter,
    NextChapter,
    SetTranslation(String),
    /// Switch to the n-th installed translation (⌘1…⌘9).
    TranslationNumber(usize),
    ToggleParallel,
    /// Do something (a jump), then show it with the parallel translations.
    OpenInParallel(Box<Action>),
    /// Select a verse and compare it across translations.
    CompareVerse(VerseRef),
    AddParallel(String),
    /// Show another translation in a parallel column (0 is the first column after the reading one).
    ReplaceParallel(usize, String),
    RemoveParallel(String),
    Highlight(Option<HighlightColor>),
    EditNote(VerseRef),
    EditSelectedNote,
    ToggleBookmark,
    RemoveBookmark(VerseRef),
    RemoveHighlight(VerseRef),
    RemoveNote(VerseRef),
    /// Save a note's text (an empty text removes it).
    SaveNote(VerseRef, String),
    /// Delete a note item by uid (a conflict copy).
    DeleteNoteItem(String),
    /// Make a conflict copy's text the verse's note, and delete the copy.
    KeepCopy {
        verse: VerseRef,
        uid: String,
        body: String,
    },
    /// Start (or restart) a reading plan today; stop one.
    StartPlan(String),
    StopPlan(String),
    /// Tick a plan's passage (day, index) read or not.
    SetPlanRead {
        plan: String,
        day: u16,
        passage: u8,
        read: bool,
    },
    SetPlanReminder(bool),
    SetReduceMotion(bool),
    /// Erase every highlight, note, bookmark, plan and the history (after a backup).
    EraseStudy,
    /// Show the data folder in the file browser.
    RevealDataDir,
    /// Focus mode: full screen with the chrome out of the way.
    ToggleFocus,
    /// Open the text & layout popover (⌘T).
    TextFormat,
    /// Forget the reading history from this time (Unix seconds) on.
    ClearReading(i64),
    /// Bookmark a verse, or remove its bookmark.
    ToggleBookmarkAt(VerseRef),
    /// Copy the selected verses' text; `Copy` adds the reference.
    CopyText,
    Copy,
    /// Open the verse menu at a point (a right-click on the selection).
    OpenVerseMenu(eframe::egui::Pos2),
    /// Make an image of the selected verses to share.
    ShareImage,
    Compare,
    /// Open or close the Ask panel (⌘J).
    ToggleAsk,
    /// Ask, with a conversation from Recents to carry on, or a new one.
    OpenChat(i64),
    NewChat,
    DeleteChat(i64),
    /// Open Ask with the selected verses as its context.
    AskAboutSelection,
    ClearSelection,
    SelectNext,
    SelectPrev,
    OpenPalette,
    OpenSearch(Option<String>),
    /// Move to the next (1) or previous (-1) search result.
    SearchStep(i32),
    ClearSearch,
    ToggleSidebar,
    SidebarTab(SidebarTab),
    OpenSettings(SettingsTab),
    /// Leaves Settings for the page it was opened from.
    CloseSettings,
    FontSize(f32),
    FontReset,
    /// System, Light or Dark, synced.
    SetScheme(Scheme),
    /// One of the six themes, synced.
    SetTheme(ThemeName),
    /// Light to Dark or back, keeping the theme.
    CycleTheme,
    ToggleMode,
    ToggleRedLetters,
    Export(ExportKind),
    ImportStudy,
    ImportTranslation,
    /// Remove an imported or online translation.
    RemoveTranslation(String),
    /// Show a page in the main area (the reader, the library, a study collection).
    OpenPage(crate::pages::Page),
    /// Open a library item by its uid (or the start of it, as a citation gives it): a sermon on
    /// the library page at that moment, a verse's own note in the notes page.
    OpenItem {
        uid: String,
        at_ms: Option<u64>,
    },
    /// The library page, searched for a reference ("Romans 8:28"): the items anchored there.
    OpenLibrary(String),
    /// Keep an Ask answer as a note in the library, anchored to the passages it cites.
    SaveToLibrary {
        text: String,
        ranges: Vec<(VerseRef, VerseRef)>,
    },
    /// Download the library as a zip (Settings → Account).
    ExportLibrary,
    ShowShortcuts,
    ScrollLines(f32),
    ScrollPages(f32),
    Toast(String),
    /// Start recording a sermon from the microphone or the computer's audio.
    StartRecording(linger_core::recording::Source),
    StopRecording,
    /// Open About This Recording for a recording, by its id.
    EditRecording(String),
    /// The Linger account: browser sign-in (and giving up on it), sign-out, fetching the plan
    /// again, Pro checkout (yearly or monthly), the Stripe portal, and deleting the account.
    SignIn,
    CancelSignIn,
    SignOut,
    /// Sign out without waiting for changes the library hasn't received (they are lost).
    SignOutNow,
    /// Try the library's waiting changes again (the sidebar footer's Retry).
    RetrySync,
    /// Show the library consent sheet again, to turn sync on.
    TurnOnSync,
    RefreshAccount,
    Upgrade {
        yearly: bool,
    },
    ManageSubscription,
    DeleteAccount,
    /// Development builds: sign in with a pasted Clerk token (empty forgets it).
    SetDevToken(String),
    /// The one-time Ask consent sheet was accepted.
    AcceptAskConsent,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SidebarTab {
    #[default]
    Books,
    Search,
    Study,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Reading,
    Appearance,
    Translations,
    Shortcuts,
    Data,
    Account,
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Json,
    Markdown,
}
