# M8: Web

Status: spike done (pass in Chrome and Firefox, see below); the app not started. Depends on: M2, and M6 for handoff. Design: [library.md](library.md#web-reader-and-ask).

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

### Findings (2026-10-20): pass in Chrome and Firefox; Safari pending

Measured in headless Chrome 155 and Firefox 158 on Linux (16 cores, the orb) by the spike's test page,
`crates/web/www/index.html`, which times every call with `performance.now()` and leaves the numbers in
`window.results`. Build: `cargo build -p linger-web --release --target wasm32-unknown-unknown`, then
`wasm-bindgen --target web --out-dir crates/web/www/pkg …` (0.2.129; the workspace release profile,
no `wasm-opt`), and serve the repository root so `/data/…` resolves. Safari is still to be checked
on a Mac.

| Criterion | Chrome | Firefox |
| --- | --- | --- |
| Wasm binary (limit 3 MB compressed) | 704,867 B raw, **262,127 B** gzip -9 | same |
| John 3 from the BSB | fetch 1,023,985 B, decode 34 ms, chapter model (36 verses) 0.3 ms; renders with its heading, verse numbers and paragraphs | decode 55 ms, chapter 0 ms |
| Search (limit 50 ms) | warm **1.5–2.4 ms** median (max 2.6) over "love" (478 hits), "living water", "faith hope love", "righteousness of god", "born again"; the first search, which builds the index, 52 ms | warm **2–3 ms** (max 4); first 75 ms |
| `cross_references` John 3:16 | 5 links; first call 89 ms including the 1,168,870 B cross-reference decode, warm 0.2 ms | first 144 ms, warm 1 ms |
| Also | wasm init 7.5 ms; names (227,573 B) decode + `people_and_places` 37 ms; original (4,227,648 B) decode + `original_words` 354 ms | init 51 ms; 42 ms; 458 ms |

Only the very first search is over 50 ms, and that is the index build; the app builds the index when
the translation loads. Nothing had to be made lazier or cut: the data stays as it is, fetched per file.

What the build needed, all in `crates/core`:
- The `native` feature, on by default, gating what core has today: `rusqlite`, `ureq`, `hound`,
  `zip`, `flate2` and the zstd C library (`keyring` was already a macOS-only dependency). `sync`, and so
  `convex`, imply it. The pure-Rust `ruzstd` decoder is always on; the C library only encodes.
- Data behind `bundled::Source`: `include_bytes!` under the `bundled` feature, otherwise whatever the
  host installs (`bundled::install`), which the page fills with fetched blobs.
- `api::Http`, with `Ureq` as the native implementation; `Api::with_http`, and `assistant`'s
  `Backend::Direct` carries its `Http`. `HighlightColor` moved from `study` (native) to `records`, and
  the clock and civil-date functions `study` and `transcribe` share moved to `time.rs`, on `web-time`
  under wasm.
- `uuid` takes its randomness from `crypto.getRandomValues` through its own `js` feature, which is
  one feature on a dependency we have rather than getrandom's `wasm_js` backend and its `RUSTFLAGS`
  cfg. It compiles; Ask, its only caller, didn't run in the spike.
- `crates/web` depends on core with `default-features = false`, and is a workspace member but not a
  default member, so the desktop, CLI and iOS builds are unchanged.

Smaller than item 3 on purpose: `crates/web` exposes the chapter model, search, references, citation
segments and `run_tool` for Ask's device-side tools, which is what the pass criteria exercise. Two
things wait for the app, with a decision each:
- **The `fetch` `Http`.** `Http::send` is synchronous because Ask reads the event stream as it
  arrives and the native loop blocks on it. A browser implementation either runs the loop in a Web
  Worker over synchronous `XMLHttpRequest`, or `assistant` grows an async entry point for wasm.
- **The web `LibraryBackend`.** `sync.rs` is native-only (SQLite). The web's `LibraryBackend` over the
  Convex JavaScript client is new code with nothing in the spike's criteria to check it against, so
  it belongs with the reader that uses it.

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
