# The iOS app's design

Status: agreed with the owner on October 22, after six rounds of a SwiftUI prototype on the iPhone 17
Pro Simulator (iOS 26.3). This document owns how the iOS app looks and behaves. The work to build it
is M3b in [m3-ios-app.md](m3-ios-app.md#m3b-the-redesign); what the library holds is
[library.md](library.md).

The prototype is a throwaway app at `/Users/wescole/Projects/linger-ios-prototype` on the owner's Mac,
outside the repository. `-layout final -topbar 2` is the agreed design; the other layouts (the drawer,
round 3's tabs, variations a to d) are kept only for comparison. Its screenshots are the reference for
the build. Its data and answers are canned.

## The idea

Three things matter, and the app is built around them: **reading**, the **library**, and **asking**.
The library is each person's own study context. It holds what they add (sermons, notes, photos of
pages and handwriting, slides, imports) and the reference works that come with Linger (the M7 set),
in one place with one search. Ask answers from the Bible and that library and says where each claim
came from.

The tone is calm and editorial and matches the desktop app: its design tokens
(`apps/desktop/assets/design-tokens.json`) exactly, through the shared themes in
[Appearance](#appearance). Literata for scripture and titles, Inter for UI text, hairline dividers, the
theme's accent used sparingly, no sparkle icons.
Liquid Glass is only on controls that float over content: the tab bar, the top bar's field and +, the
verse row, the capture menu, the margin popover and the return bar. Lists, the chat, Library, item
pages and Settings are plain.

## The frame

```diagram
┌──────────────────────────────────────────┐
│ [ 🔍 Romans 8 · KJV              ]  (+)  │  jump field, capture
│                                          │
│              the reader                  │
│                                          │
│ ( Bible  Library  Settings )      ( 💬 ) │  tabs, Ask
└──────────────────────────────────────────┘
```

- **Tabs:** Bible, Library and Settings, in the system iOS 26 `TabView`. The bar minimizes on scroll
  down (`tabBarMinimizeBehavior(.onScrollDown)`), so reading keeps the screen.
- **Ask** is the separated round button at the trailing end of the tab bar, `Tab(role: .search)`.
  Its accessibility label is "Ask", not "Search".
- **Add** is a round glass + in the top toolbar on Bible, Library and item pages. It is not a tab:
  tabs are places, and Add is an action.
- There is no drawer and no ☰.

## Bible

**The top bar** is one glass field, "Romans 8 · KJV", and the +. Nothing else: no ‹ › arrows, no Aa
(text settings are in Settings), no separate search.

Tapping the field opens it with the keyboard up. The field and the list under it are one tool:

- Empty: Recent places (not the chapter on screen), then an Old / New Testament control that starts on
  the testament being read, then the books as plain rows with grey chapter counts.
- A book row expands in place into a six-column grid of its chapters, 44 × 44 pt each, the current one
  outlined. One book is open at a time. The current book starts open and scrolled into view, so the
  next chapter is two taps away.
- Typing filters the same list. A book name narrows it to that book with its grid open. A reference
  ("jn 3", "rom 8:28") puts "Go to John 3" on top. Words give verse matches with the words in bold,
  then "In your library". Return takes the top result.
- A small "KJV ⌄" inside the open field changes the translation without closing it.
- Scrolling the list drops the keyboard. Opening and closing don't cross-fade over the text: the field
  grows in place and the list slides down on its own background.

**Chapters** turn by pulling past an end. Past the last verse, a glass capsule with a filling ring
says "Romans 9", then "Release for Romans 9" with a light haptic; letting go turns the chapter.
Pulling down above the heading goes back to the top of Romans 7 the same way. Only a lifted finger
turns: a fling that bounces past the end doesn't. The chapter ends with a quiet grey "Pull up for
Romans 9", which teaches the pull and also turns on a tap. There are no other links; VoiceOver gets
Previous and Next chapter as actions on the heading. That and the field are the only paging.

**Margin marks.** One small grey glyph beside each verse that has library items, with no counts; a
bookmark adds a small bookmark glyph in the accent. Tapping the library glyph opens a glass popover anchored to
the verse: "Romans 8:28 ›" (which opens the verse page), Your library, then Reference, each row with
an Ask button. A quiet "3 in your library" under the chapter number opens Library filtered to the
chapter.

**Selecting.** Tapping a verse underlines it; tapping more extends the range. The tab bar hides and a
glass row takes its place:

```diagram
 ●  ●  ●  ●  ●     ···     💬 Ask
```

- Five highlight colours, 20 pt dots inside 44 pt targets, about 24 pt apart. No dividers, no label:
  the underline and the verse numbers say what is selected.
- **···** holds, in a fixed order (`menuOrder(.fixed)`): Study this verse, then Compare, Bookmark (or
  Remove bookmark) and Share. Copy is in the share sheet.
- **Ask** opens Ask with the selection quoted.
- The toolbar + follows the selection: its menu reads "Anchored to Romans 8:28".
- If the row would cover the tapped verse, the text scrolls it up.

**The verse page** (Study this verse, or the popover's header): the verse in large serif, the colour
dots, then Your library, Reference, Cross references and notes on that verse. The tab bar hides. Its
own "Ask about this verse" bar opens the same Ask sheet, so the page's sections stay in place.

## Ask

- A **full-height sheet** over the reader, from the Ask button, a verse row, the popover or a page.
- **Context** at the top: the selection quoted with ×, or "Reading Romans 8" with nothing selected, or
  the item being asked about. A scope control: "Bible + library" (the default) or "Bible only".
- **Empty:** three suggested questions as plain rows, drawn from the context and the library.
- **Answering:** a grey user bubble, the answer in serif with inline citations in the accent, and one grey line
  saying what it read ("Read 1 sermon and 1 note from your library"). A stop button while it writes.
  The answer appears without cross-fading over the suggestions.
- **Sources** come in three groups: From the Bible, From your library, From reference works. Small
  actions under them: copy, save to library, share, thumbs.
- **Tapping a source** opens a peek card in the chat with the verse text or the item's excerpt, and
  "Open in Bible" (or "Open"). Opening collapses the sheet and leaves a return bar above the tab bar,
  "‹ What does 8:28 really promise?" (`tabViewBottomAccessory`), which brings the chat back as it was.
  While it shows, the field reads "‹ Romans 8 | Genesis 50": the back link first, in the accent, then the place.
- **History:** a clock button in the header opens past chats grouped by day, with search.
- **Out of questions:** plain text above a disabled composer, with a link to Pro. Reading, notes and
  the library keep working.

## Add

The + opens a small glass menu: Note, Photo, Record, Import, and "Anchored to Romans 8:28 · Change".
With nothing selected it anchors to the chapter on screen. Library's home also has a Record / Photo /
Note / Import row. Cancelling a note keeps the verse selection, so another action can follow (the
owner's decision, October 22).

## Library

- A large serif title, "7 yours · 9 reference works", and "Search your library".
- The capture row, then filters: All, Yours, Reference, and the kinds (Sermons, Notes, Photos, Marks).
  Highlights and bookmarks are the Marks filter; there is no separate Notes or Highlights screen.
- **On Romans 8:** items about the chapter being read, yours and reference together, each with a grey
  origin label ("Your sermon", "Your photo", "Reference"). Then Recent by day, then the reference works.
- **Item pages:** a sermon has its title, speaker line, Ask about this, summary, passage links and a
  timed transcript; times are text, not play buttons. A photo has the image, its anchor with Change,
  Ask about this, the text read from it and its passages. A reference entry has the work's name, the
  passage, the text and a public-domain credit line.

## Settings

Reading (Appearance, font, size with a sample line), then the account: email, plan (Free or Pro), questions
left "23 of 30 this month", Upgrade to Pro, sync, storage, export, Sign out, and Delete account in red.
The Pro sheet shows "Couldn't load prices. Try again" when StoreKit returns no products, never an empty
section.

## Appearance

Settings › Appearance, the same on the Mac and the iPhone, and synced between them (the owner's
decisions, October 22):

- **Color scheme:** System (the default), Light or Dark, as three cards with a small drawing of the
  reader; System's is split light and dark.
- **Themes:** Linger (the default), Paper, Forest, Oxblood, Plum and Graphite. Each is a light and a
  dark palette; the scheme picks which shows. A card shows both as round swatches, the one in use
  ringed with a sun or moon badge. No custom themes.
- Linger is the desktop's `color.light` and `color.dark` exactly. Paper is `color.sepia` and a warm
  dark (paper #1C1915, ink #E8DFCF, accent #D9A86C). Forest (#2F6B4F, dark #7CC4A0), Oxblood
  (#8E2B2B, dark #E8908A), Plum (#6B3F7A, dark #C9A2DA) and Graphite (#4A4D55, dark #B7BBC6) are
  Linger's neutrals with that accent. Verse numbers follow the accent. Highlights keep their hues.
- The themes live in `linger-core`, so both apps draw from one definition, and the scheme and theme
  sync as one `preferences` record, last writer wins. The desktop's Light/Sepia/Dark and accent
  settings carry over: Sepia becomes Paper, an accent becomes its theme, Dark becomes the Dark scheme.

## The deployment target

iOS 26.1 and later only, with no fallback for older versions (the owner's decision, October 22). The
design needs iOS 26 APIs: Liquid Glass, the search-role tab, and `tabViewBottomAccessory` from 26.1.
Linger has no iOS users yet.

## Not in this design

- Audio playback of sermons: later, by the owner's decision.
- Slide photos taken during a recording, and real Import sources: they appear only as menu entries
  until M4 and a milestone for imports exist.
- iPad layout: the same tabs first; a sidebar layout is later.

## Open questions

- The navigator scrolls to the current book's grid on open, which hides Recent until you scroll up.
  Kept for now.
- A long selection whose start has scrolled away has nothing saying where it starts. Left out until it
  comes up.
- The Ask button's bubble draws black, as the system draws that slot. It could take the accent.
