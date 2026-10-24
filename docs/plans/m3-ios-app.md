# M3: The iOS app

Status: M3a in progress (October 20), run ahead of M1b and M2 by the owner's decision: a working
product on Mac and iOS as soon as possible. M3b follows M1b and M2. Everything stays on Clerk's
development instance and dev Convex until the production cut-over, which is parked until the iOS app
runs ([m1](m1-library-foundation.md#before-any-code-ships)). Depends on: M1 (M3a); M1b and M2 (M3b).
Design: [library.md](library.md), [backend.md](backend.md#accounts).

## The split

**M3a, on M1's surface.** The FFI growth below (1–3, 5–7: tokens, `Study` and its observer, sync,
`find_all` and citation segments, `me()` / `deleteAccount()`, Keychain secrets), then the app:
sign-in through the Clerk iOS SDK (email code and passkeys, development instance), the reader with
selection and the six highlight colours, bookmarks, the verse note sheet and the notes list, Recents,
the library and Ask consent sheets, sync status, Settings → Account with deletion, and StoreKit 2
last (it needs the proxy, not M1b or M2). Ask ships in M3a only if the device-side tools the desktop
had before M1 cross the FFI cheaply; otherwise it waits for M3b. Build order: the FFI with Rust tests
and `LingerCoreTests`, sign-in, the reader and notes with sync, then the rest. Every UI step is
checked on the simulator.

**M3b, after M1b and M2.** The app's redesign around Bible, Library and Ask
([ios-design.md](ios-design.md)), with the library's item views (app 4, without playback) and Ask with
the library tools (FFI 4, app 5), plus the Done-when items that need them. The work is
[below](#m3b-the-redesign); where the redesign and "The app" disagree, the redesign wins.

**Proxy for M3a.** `me()`, deletion, Ask and StoreKit go to the proxy, and the production proxy
verifies production tokens, so the simulator talks to a proxy on dev Clerk and dev Convex: `pnpm dev`
on the Mac (`127.0.0.1:3000`, which the simulator reaches), set by a build setting as the desktop's
`LINGER_API_URL` is.

**Goal.** A signed-in iPhone and iPad app that reads, takes notes, highlights, asks questions over the
library, and stays in sync with the Mac. It reaches TestFlight at the end of M3. Capture comes next:
photos in M4, recording in M5.

**Where it starts.** `apps/ios` is a SwiftUI proof of concept: the reader, a translation picker,
previous and next chapter, and a typed reference jump, on `crates/ffi` (UniFFI 0.32), which exposes
`BibleLibrary` (translations, chapters and search, with a `LibraryObserver`), books and references. There's no sign-in, no study data, no Ask, no Keychain and
no entitlements yet.

## crates/ffi

The iOS app runs the same Rust as the desktop for everything that isn't UI. Swift holds the views and
the platform APIs it must (Clerk, StoreKit, AVPlayer, the keyboard) and decides nothing: a rule that
Swift seems to need goes into core and crosses the FFI. The FFI grows:

1. **Tokens.** A foreign-implemented `TokenProvider` trait (`fn token() -> Option<String>`,
   `fn refresh()`) that Swift implements with the Clerk SDK; core wraps it as an `auth::TokenSource`.
2. **The study store and cache:** `Study` (open, highlights, bookmarks, notes as items, Recents, the
   library list and items, `forChapter`), with a `StudyObserver` callback when `generation` moves.
3. **Sync:** `Sync::start(data_dir, token_provider)` over the `ConvexBackend`, the same engine as the
   desktop's, `status()`, and the observer. The tokio thread lives inside Rust, as on the desktop.
   Built as `LibrarySync.start(study, convexUrl, tokens, observer)`: it reads the engine's writes into
   the `Study`, whose observer then fires, as the desktop's `library_sync` does each frame.
4. **Ask:** `Conversation` (new, from saved JSON, `ask(question, context, mode, observer)`), streaming
   `AskEvent`s (text, tool, thinking, allowance) through a callback, with the device-side tools, the
   public-domain gate and the library tools exactly as on the desktop.
5. **References:** `find_all`, and citation parsing (`segments`) so Swift renders chips the same way.
   M3a ships `reference_segments` (over `find_all`); `citation_segments` comes with Ask, its only user.
6. **The API:** `me()`, `deleteAccount()`, and `attachAppleTransaction(jws)`.
7. **Secrets** move to the Keychain on iOS (`secrets.rs`; the README lists this as not done).
   Not in M3a: nothing on iOS writes through `SecretStore` (Clerk keeps its session in its own
   Keychain item, and the app sets no translation keys), so it waits for the first secret that does.

`scripts/build-ios-core.sh` and the CI `ios` job keep building it. Check the static library's size with
`original`, `crossrefs` and `names` now used by the Ask tools (about 6 MB compressed on top of the
translations), and that the app launches to text as fast as now.

## The app

1. **Sign-in:** the Clerk iOS SDK with email code and passkeys (native Sign in with Apple and Google
   once they are turned on; [backend.md](backend.md#clerk)), on a first
   screen that explains the library in one sentence. Sign-out (which waits for the outbox, then clears
   the cache), and **Delete account** in Settings. Confirm that deleting a Clerk user revokes its Sign in
   with Apple token (Apple TN3194); if it doesn't, store the token and revoke it in the purge.
2. **The reader:** tap a verse to select it (drag for a range), then highlight in the six colours,
   bookmark, write a note or copy. Margin marks for notes, bookmarks and library items, matching the
   desktop's meanings. The chapter's "In your library" line.
3. **Notes:** the verse note sheet, and a notes list with search.
4. **The library:** items by kind and date, and item views. A sermon shows its summary, outline and
   transcript, and plays with `AVPlayer` straight from its Convex link (range requests), with lock-screen
   controls.
5. **Ask:** a sheet from the toolbar, streaming answers with verse and item chips, Deep study for Pro,
   Recents, and the allowance message on 402 and 429.
6. **Settings → Account:** plan, questions and sermon hours left, storage used, Manage subscription,
   Export, Sign out, Delete account.
   M3a leaves Export out: no Done-when needs it. Settings also shows the sync status and Retry, and
   anything the study store refuses.
7. **Consent sheets:** the library sheet at first sign-in and the Ask sheet at first question
   ([backend.md](backend.md#privacy-and-consent)).
   M3a has the library sheet (its words are core's `privacy`, shared with the desktop); the Ask
   sheet comes with Ask, since the iPhone asks no questions in M3a. Recents is read-only until then:
   conversations from the Mac, with verse citations that open their chapter.
8. **Sync status** in the library tab, as on the desktop.
9. **Handoff basics:** opening a citation link from the Mac isn't in scope; reading position sync is M6.

## M3b: the redesign

The design is [ios-design.md](ios-design.md). Today's app (`apps/ios/Linger`, about 1,000 lines) is
one reader with sheets for Notes, Recents and Settings; the redesign replaces its shell and reuses its
pieces.

**Where today's views go.**

| Today | In the redesign |
|---|---|
| `LingerApp.Root` → `ReaderView` | A `TabView`: Bible, Library, Settings, and Ask in the search-role slot |
| `ReaderView`'s toolbar (‹ › arrows, translation picker, typed jump) | The jump field with the book and chapter navigator; end-of-chapter links |
| `ReaderView`'s selection sheet and `VerseRow` marks | The glass verse row, the ··· menu, margin glyphs and the margin popover |
| `Notes.swift` (verse note sheet, notes list) | The note sheet from the + menu; the list becomes Library's Notes filter |
| `Recents.swift` (read-only chats) | Ask's history list, and Ask itself |
| `SettingsView`, `ProSheet` | The Settings tab, with Reading on top; the Pro sheet's no-prices state |
| `ConsentSheet`, `SignInView`, `Store` | Unchanged |

**The FFI grows** (adding to 1–7 above):

8. **Library items:** `Study` gains the cache calls the desktop's Library page and chips use since
   ecd2896: `StudyStore::items`, `item`, `body` and `spans_in_chapter`, and `save_note_item` for Save
   to library. Filtering by kind and origin stays in core, not Swift.
9. **Search:** `LibrarySync` exposes the library search the Ask tools already use
   (`chat::UserLibrary::search`, which answers from the cache when offline, adba3b9), so the jump
   field shows verses from `BibleLibrary.search` and library items together.
   `suggest_references` already gives "Go to John 3".
10. **Ask:** FFI 4 as planned. The "Read 1 sermon and 1 note" line is built from the `Read` events the
    device reports as its tools read a passage or a library item (core's `assistant::Event::Read`),
    with the answer's citations; the proxy and contract don't change. The scope control ("Bible
    only") keeps the library tools from running, with no contract change either: the proxy still
    sends the tools, but the question's note says nothing of the library and the device answers a
    library tool call with an error saying the user asked for the Bible only. Built in `ask.rs`:
    `Conversation` (`ask`, `stop`, `open` from Recents, saved there after each answer),
    `ask_sources_line`, `ask_suggestions` and `ask_consent_text`.

**Appearance, shared with the Mac** ([ios-design.md](ios-design.md#appearance)). Built before the
iOS screens, in this orb:

11. **Themes in core:** `linger_core::appearance` holds the six themes' light and dark palettes,
    read from the token file, and the FFI hands Swift the palette for a theme and scheme. The
    desktop's `theme.rs` takes its palettes from it instead of its own accent mixing.
12. **The `preferences` record:** one per user (`uid` "preferences") with `scheme` and `theme`,
    declared in `records.rs` and synced through the feed, `library/preferences.set` and the outbox
    like the others, last writer wins. Purge and export include it.
13. **The desktop's Settings › Appearance** becomes the scheme cards and theme grid, writing the
    record; its old `theme`, `theme_system` and `accent` settings migrate once.

**Outside this milestone, and what shows until it lands.**

- **Reference works** in Library, the popover and Ask's third source group need M7 loaded and
  `ask/6` ([m7](m7-outside-sources.md#ask)). Until then those sections are absent, not empty.
- **Photo** in the + menu is M4; **Record** is M5 on iOS. **Import** has no milestone yet. Until each
  lands its menu entry is hidden.
- The Marks filter and the bookmark glyph use what M3a already syncs.

**Build order:** appearance (11–13, here), then on the Mac: the deployment target (below), the tab shell with today's reader inside it, the jump
field and navigator, the verse row, ··· and the + menu, the margin popover and verse page, Library
from the cache (FFI 8, 9), Ask with sources, peek and the return bar (FFI 10), then Settings with Appearance. Each step
is checked on the simulator against the prototype's screenshots.

**Deployment target:** iOS 26.1, raised from 17, with no fallback for older versions (the owner's
decision, October 22; [ios-design.md](ios-design.md#the-deployment-target)). The glass, the
search-role tab and `tabViewBottomAccessory` need it, and Linger has no iOS users yet.

**Tests:** extend LingerUITests with the prototype's script: jump to John 3 through the navigator, tap
Romans 8:28, highlight it, ask a question against a mock proxy, open a source and come back by the
return bar, add a note through the +, and open the verse page. `LingerCoreTests` covers FFI 8–11.

## StoreKit 2

- Products `pro.monthly` and `pro.yearly` in one group; a StoreKit configuration file for Xcode testing.
- Purchase with `appAccountToken` from `/v1/me`, then post the JWS to
  `/v1/billing/apple/transactions` (built server-side). Restore Purchases, and `Transaction.updates` on
  launch.
- The Pro sheet states allowances in questions, sermon hours, photos and storage
  ([backend.md](backend.md#plans-and-limits)), as guideline 3.1.2(c) asks.

## project.yml

- Capabilities: Keychain sharing (for Clerk), In-App Purchase, and Associated Domains with
  `webcredentials:` the Clerk Frontend API's host, which Clerk's iOS SDK needs for passkeys
  (`elegant-mink-67.clerk.accounts.dev` in M3a, `clerk.trylinger.com` from the cut-over). Sign in
  with Apple joins when Apple sign-in is turned on in Clerk.
- Usage descriptions arrive with the features that need them (camera and photo library in M4,
  microphone in M5).
- The deployment target is iOS 26.1 from M3b ([above](#m3b-the-redesign)), so SpeechTranscriber (M5)
  needs no availability check.

## Tests

- `LingerCoreTests` (Swift, through the bindings): a token provider feeds sync; a note written through
  `Study` appears in the observer; an Ask conversation against a mock proxy streams text and runs a tool.
- Rust: FFI round-trips for the new records; `Conversation` with a fake proxy.
- UI tests for sign-in (against Clerk's test mode), selecting a verse and writing a note, and an Ask
  question with a citation chip.
- StoreKit: a hosted test target, LingerTests (storekitd accepts only the app's identity; xcodebuild
  doesn't load a scheme's StoreKit configuration into an XCUITest), proves purchase and refund against
  `pnpm dev`; renewal and revoke by a scratch run; the sandbox purchase is manual. LingerTests needs
  `pnpm dev` with `APPLE_ENVIRONMENT=Xcode` and `LINGER_TEST_TOKEN`, and skips without them; CI
  doesn't build it. Debug simulator builds sign with `Linger-Simulator.entitlements`, which adds
  `get-task-allow`; without it storekitd refuses the test session.

## Done when

**M3a:**

- A note or highlight made on the iPhone simulator appears on the Mac within a second, and the other
  way round. **Proven on the simulator** (8d85783, iPhone Air, dev Convex): a highlight tapped on the
  phone was on the Mac 0.77 s later (an upper bound, including the CLI sync), and a bookmark synced
  on the Mac was on the phone's screen 0.17 s after the Mac's sync returned. On a device with
  TestFlight later.
- Sign-in and sign-out work through Clerk's development instance. The device's cache records whose
  library it holds (the token's `sub`), and sync clears it before starting as a different account, so
  a session that ended without signing out (expiry, a lost keychain) doesn't hand one account's
  library to the next; a cache from before this has no owner and is adopted. **Proven on the simulator**
  (8d85783, iPhone Air): sign-out cleared study.sqlite to zero rows and a relaunch opened on the
  sign-in screen; signing in again refilled the same counts as the Mac (40 highlights, 11 bookmarks,
  37 note items). The headless XCUITest (LingerUITests) signs in by email code. On a device later.
- Account deletion from the iPhone removes everything ([backend.md](backend.md#deletion)). **Proven on
  the simulator** (207fadd, iPhone Air, dev): a throwaway user (user_3KymVPc7GS0rt1Z7pyDEIGvQWzQ)
  with 1 highlight and 1 note deleted its account from Settings; afterwards Clerk answered 404 for
  the user, dev Convex held 0 of its records and the device's study.sqlite was empty. On a device
  later.
- A sandbox purchase on the iPhone unlocks Pro on the Mac. **Built; the Xcode-configuration path is
  proven on the simulator** (iPhone 17 Pro, dev): LingerTests buys pro.yearly, the proxy makes the
  test user Pro from App Store, and a refund makes it Free; a scratch run renewed it (a new
  transaction on the same original, `renewsAt` moved) and expired it (Free). StoreKit Testing has no
  separate revoke: a refund is the client-side revocation, and Family Sharing's REVOKE comes only by
  App Store notification, which a local proxy can't receive. Waiting on Wes: the sandbox purchase
  itself (Paid Applications agreement, the two products in App Store Connect, a sandbox tester).

**M3b:**

- The redesign's script (above) passes on the simulator, and its screens match the prototype's.
- Ask on the iPhone cites a sermon recorded on the Mac and opens its transcript at the cited moment
  (playback is later).
- A TestFlight build is in testers' hands, with review notes explaining why sign-in is required.

## Risks

- **App Review and sign-in.** The library is the justification ([backend.md](backend.md#plans-and-limits));
  the review notes and the first screen must make that obvious.
- **UniFFI callbacks across threads.** Ask and sync call back from Rust threads; Swift hops to the main
  actor in every observer.
