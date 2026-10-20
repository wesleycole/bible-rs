# M8: Web

Status: not started. Depends on: M2, and M6 for handoff. Design: [library.md](library.md#web-reader-and-ask).

**Goal.** Sign in at `app.linger.app` in any modern browser, read the Bible, edit notes and highlights,
and ask questions over your library. No recording or capture on the web.

## Spike first: linger-core on WebAssembly

Two days, before any UI:
1. A `native` feature in `crates/core` (on by default) gating every native-only dependency listed in
   [library.md](library.md#web-reader-and-ask) (storage, networking, audio, images, transcription and the
   `sync` feature). Without it, decoding uses the pure-Rust `ruzstd`, and `uuid` uses getrandom's
   `wasm_js` backend.
2. Data loading behind a trait: `include_bytes!` natively, fetched blobs in wasm.
3. `crates/web` (wasm-bindgen) exposing the reader's chapter model, search, references, citation parsing
   and the device-side Ask tools.
4. **Pass:** John 3 renders from the BSB in a test page, search answers in under 50 ms, and
   `cross_references` runs, in Chrome, Safari and Firefox, with a wasm binary under 3 MB compressed.
5. **Fail:** port the chapter decoding and search to TypeScript instead, and run the Ask tools that need
   original-language data through a Convex action.

## The app (`web/`)

- **Stack:** Vite, React and TypeScript; `ConvexProviderWithClerk`; Clerk's React SDK; the wasm package.
  Deployed as its own Vercel project; `app.linger.app` from M9, a Vercel URL before.
- **Data:** the bundled translations and Ask data fetched on first use from the web project's static
  files and kept in Cache Storage, versioned by content hash.
- **Reader:** chapter view in HTML and CSS with Literata, the same light, sepia and dark themes, verse
  selection, highlights, bookmarks and notes (written through Convex mutations directly; the web has no
  outbox and says so when offline), the margin and "In your library" marks, ⌘K references and search.
- **Ask:** the tool loop in TypeScript (`web/src/ask.ts`), rounds to `/v1/ask` (CORS,
  [backend.md](backend.md#post-v1ask)), tools through wasm and `library.search`, streaming answers with
  chips, Recents, Deep study for Pro.
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
