# M3: The iOS app

Status: not started. Depends on: M1, M1b, M2. Design: [library.md](library.md), [backend.md](backend.md#accounts).

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
4. **Ask:** `Conversation` (new, from saved JSON, `ask(question, context, mode, observer)`), streaming
   `AskEvent`s (text, tool, thinking, allowance) through a callback, with the device-side tools, the
   public-domain gate and the library tools exactly as on the desktop.
5. **References:** `find_all`, and citation parsing (`segments`) so Swift renders chips the same way.
6. **The API:** `me()`, `deleteAccount()`, and `attachAppleTransaction(jws)`.
7. **Secrets** move to the Keychain on iOS (`secrets.rs`; the README lists this as not done).

`scripts/build-ios-core.sh` and the CI `ios` job keep building it. Check the static library's size with
`original`, `crossrefs` and `names` now used by the Ask tools (about 6 MB compressed on top of the
translations), and that the app launches to text as fast as now.

## The app

1. **Sign-in:** the Clerk iOS SDK with native Sign in with Apple, Google and email code, on a first
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
7. **Consent sheets:** the library sheet at first sign-in and the Ask sheet at first question
   ([backend.md](backend.md#privacy-and-consent)).
8. **Sync status** in the library tab, as on the desktop.
9. **Handoff basics:** opening a citation link from the Mac isn't in scope; reading position sync is M6.

## StoreKit 2

- Products `pro.monthly` and `pro.yearly` in one group; a StoreKit configuration file for Xcode testing.
- Purchase with `appAccountToken` from `/v1/me`, then post the JWS to
  `/v1/billing/apple/transactions` (built server-side). Restore Purchases, and `Transaction.updates` on
  launch.
- The Pro sheet states allowances in questions, sermon hours, photos and storage
  ([backend.md](backend.md#plans-and-limits)), as guideline 3.1.2(c) asks.

## project.yml

- Capabilities: Sign in with Apple, Keychain sharing (for Clerk), In-App Purchase, and Associated Domains
  for `auth.trylinger.com` (which exists from M1) if Clerk's iOS SDK needs it.
- Usage descriptions arrive with the features that need them (camera and photo library in M4,
  microphone in M5).
- The deployment target stays iOS 17; SpeechTranscriber (M5) is used only on iOS 26 and later.

## Tests

- `LingerCoreTests` (Swift, through the bindings): a token provider feeds sync; a note written through
  `Study` appears in the observer; an Ask conversation against a mock proxy streams text and runs a tool.
- Rust: FFI round-trips for the new records; `Conversation` with a fake proxy.
- UI tests for sign-in (against Clerk's test mode), selecting a verse and writing a note, and an Ask
  question with a citation chip.
- StoreKit Testing in Xcode: purchase, renewal, refund and revoke move the plan on `/v1/me`.

## Done when

- A note written on the iPhone appears on the Mac within a second, and the other way round.
- Ask on the iPhone cites a sermon recorded on the Mac and plays it from the cited second.
- A sandbox purchase on the iPhone unlocks Pro on the Mac.
- Account deletion from the iPhone removes everything ([backend.md](backend.md#deletion)).
- A TestFlight build is in testers' hands, with review notes explaining why sign-in is required.

## Risks

- **App Review and sign-in.** The library is the justification ([backend.md](backend.md#plans-and-limits));
  the review notes and the first screen must make that obvious.
- **UniFFI callbacks across threads.** Ask and sync call back from Rust threads; Swift hops to the main
  actor in every observer.
