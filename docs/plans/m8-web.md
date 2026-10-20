# M8: Web

Status: not started. Depends on: M2, and M6 for handoff. Design: [library.md](library.md#web-reader-and-ask).

**Goal.** Sign in at `app.trylinger.com` in any modern browser, read the Bible, edit notes and highlights,
and ask questions over your library. No recording or capture on the web.

## Spike first: linger-core on WebAssembly

Two days, before any UI:
1. A `native` feature in `crates/core` (on by default) gating every native-only dependency listed in
   [library.md](library.md#web-reader-and-ask) (storage, networking, audio, images, transcription, the
   `convex` crate and the Apple encoder). Without it, decoding uses the pure-Rust `ruzstd`, and `uuid`
   uses getrandom's `wasm_js` backend.
2. Data loading behind a trait: `include_bytes!` natively, fetched blobs in wasm. The `Http` trait for
   `api` and `assistant`: `ureq` natively, `fetch` in wasm.
3. `crates/web` (wasm-bindgen) exposing the reader's chapter model, search, references, citation parsing,
   the Ask tool loop (`assistant.rs`) with its device-side tools, and a `LibraryBackend` implemented
   over the Convex JavaScript client passed in from the page.
4. **Pass:** John 3 renders from the BSB in a test page, search answers in under 50 ms, and
   `cross_references` runs, in Chrome, Safari and Firefox, with a wasm binary under 3 MB compressed.
5. **Fail:** fix the build: lazier data, fewer features, more fetches. There is no TypeScript port of
   core ([README](README.md#where-code-lives)).

## The app (`web/`)

- **Stack:** Vite, React and TypeScript; `ConvexProviderWithClerk`; Clerk's React SDK; the wasm package.
  React renders and holds no logic.
  Deployed as its own Vercel project; `app.trylinger.com` from M9, a Vercel URL before.
- **Data:** the bundled translations and Ask data fetched on first use from the web project's static
  files and kept in Cache Storage, versioned by content hash.
- **Reader:** chapter view in HTML and CSS with Literata, the same light, sepia and dark themes, verse
  selection, highlights, bookmarks and notes (through the web `LibraryBackend`; the web has no cache
  or outbox and says so when offline), the margin and "In your library" marks, ⌘K references and search.
- **Ask:** `assistant.rs` in wasm, rounds to `/v1/ask` through the `fetch` `Http` (CORS,
  [backend.md](backend.md#post-v1ask)), tools in wasm and `search_library` through the web
  `LibraryBackend`, streaming answers with chips, Recents, Deep study for Pro.
- **Items:** read-only views for citations: a sermon's transcript with `<audio>` playback from the cited
  second; a photo with its marks; notes are editable.
- **Account:** plan and allowances from `/v1/me`; upgrading opens Stripe Checkout; Export; Sign out;
  Delete account. The proxy allows the web origin on each of these routes
  ([backend.md](backend.md#post-v1ask)).
- **Handoff:** the web reads and writes `positions` (from M6), offering "Continue from your iPhone".
- **Auth:** `ConvexProviderWithClerk` uses Clerk's session token directly, because M1 activated Clerk's
  Convex integration ([backend.md](backend.md#convex-auth)).
- **Accessibility:** real text, so selection, find in page, zoom and screen readers work.

## Tests and done when

- Playwright: sign in with a Clerk test user, read John 3, highlight a verse and see it on the desktop,
  ask a question that cites a sermon and play it from the cited second.
- The reader's first chapter renders in under one second on a cold cache over a fast connection, and
  under 200 ms warm.
- **Done when** the web app does all of the above in current Chrome, Safari and Firefox, and its Ask
  answers match the desktop's for the same eval questions.
