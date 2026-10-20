# Linger plans

Status: October 19, 2026.

Linger is a **personal Bible context manager**. Reading the Bible is the main screen. As people read,
listen to sermons and study, they build a library: recorded sermons, photos of their handwritten
notes, of sermon slides and of the book pages they mark up, notes written while reading, and trusted
outside sources. Ask answers their questions from that library and the Bible, cites where each claim
came from, and works the same on the Mac, the iPhone and the web.

## The documents

Each fact is written in one document; the others link to it.

| Document | Owns |
|---|---|
| [backend.md](backend.md) | Accounts and sign-in, Convex auth, the Ask proxy and contract, model routing, **plans and limits**, payments, licensing of Bible text, **privacy and consent**, observability, server testing, environments |
| [library.md](library.md) | The library's data model, sync and handoff, capture (sermons, photos, notes), references and anchors, search and the library's Ask tools, outside sources, the web client, what the library costs |
| `m1-…` to `m9-…` | One implementation plan per milestone: the work, the tests, what "done" means, and the risks. They don't restate design; they link to it |

[DECISIONS.md](../../DECISIONS.md) records why the reader is built the way it is (egui, typography,
the data format, translations). The October 18 research that led to these plans is kept locally, out
of the repository; where it differs (it recommended a Rust proxy on Fly with its own auth), these plans
win.

## Roadmap

Backend phases 1–4 (the Ask proxy, desktop accounts, Pro and Stripe, sermons through the proxy) are
built; [backend.md](backend.md#whats-built) has the details. The milestones below build the library
on top of them.

| | Milestone | Delivers | Depends on |
|---|---|---|---|
| M1 | [The library foundation](m1-library-foundation.md) | The production Clerk instance and `trylinger.com` first; Effect inside Convex, the record schema generated from Rust, the sync engine in core behind `LibraryBackend`, the desktop on it, migration of notes, highlights, bookmarks and Recents | |
| M1b | [Sermons on Convex](m1b-sermons-on-convex.md) | One AAC encoder and one M4A muxer in core, parts for crash safety, the transcription workflow in Convex, recordings migrated, Pro audio kept, Vercel Blob gone | M1 |
| M2 | [Search and Ask over the library](m2-search-and-ask.md) | Chunks, the digest step and its anchors, `library.search`, contract `ask/5`, citations, the reader's library marks, export, `cargo xtask eval-ask` | M1, M1b |
| M3 | [The iOS app](m3-ios-app.md) | Sign-in, sync, the reader with notes and highlights, Ask, Recents, StoreKit, account deletion | M1, M2 |
| M4 | [Photo capture](m4-photo-capture.md) | Camera and scanner, PencilKit marks, extraction, review, sources, photo allowances | M3 |
| M5 | [Recording everywhere](m5-recording-everywhere.md) | iOS recording on core's encoder, slides attached by time, on-device transcription for Free (iOS, macOS, Linux) | M1b, M3, M4 |
| M6 | [The Mac as a capture device, and handoff](m6-mac-capture-and-handoff.md) | Continuity Camera, drag-and-drop, marks on the Mac, reading position and plan sync, `ASWebAuthenticationSession` | M4 |
| M7 | [Outside sources](m7-outside-sources.md) | Public-domain commentaries, reference works and confessions in shared tables; contract `ask/6` | M2 |
| M8 | [Web](m8-web.md) | The reader and Ask in the browser, on `linger-core` compiled to wasm | M2 (and M6 for handoff) |
| M9 | [Launch](m9-launch.md) | `api.` and `app.trylinger.com`, observability, legal answers, privacy page, App Store, Mac and web releases | all |

M7 and M8 can run alongside M3–M6. Each milestone ships to `main` and to users as it lands. The one
exception is book-page capture (M4 on iOS, M6 on the Mac), which stays out of public builds behind a
release-build switch, as the online translations are today (`LICENSED_ONLINE`), until the lawyer answers
([backend.md](backend.md#open-questions)).

```diagram
M1 library foundation ──▶ M1b sermons on Convex ──▶ M2 search and Ask ──┬──▶ M3 iOS ──▶ M4 photos ──┬──▶ M5 recording
                                                                        │                           └──▶ M6 Mac capture, handoff
                                                                        ├──▶ M7 outside sources
                                                                        └──▶ M8 web
                                                                                    all ──▶ M9 launch
```

## Where code lives

Linger is written in Rust and in TypeScript on Effect, and nothing else carries logic:

- **Logic lives in `linger-core`** when it can run on a device or in wasm: records, references, the
  sync engine and outbox, the Ask tool loop, audio encoding, framing and muxing, marks, image
  preparation, search. Desktop, iOS and web call the same code.
- **Server logic is Effect**, in the proxy and inside Convex handlers alike
  ([backend.md](backend.md#effect-in-convex)): services as layers, `Schema` at every edge, typed errors
  from one shared module.
- **Swift, React and Node are leaves.** They hold platform APIs and views (`AVAudioEngine`, PencilKit,
  SpeechTranscriber, the DOM) and hand raw data to core or to Effect. A leaf never decides anything a
  test can't reach through core or a layer.
- **The schema is written once, in Rust**, and generated into Convex validators, Effect Schemas, the
  cache and the FFI by `cargo xtask schema`; a test fails on drift ([library.md](library.md#one-schema)).
- **One door to Convex from a device:** the `LibraryBackend` trait in core, with a fake for tests
  ([library.md](library.md#the-sync-engine)). No second client, no port of the tool loop.

## Shared vocabulary

These terms mean the same thing in every plan:

- **Library**: everything personal the user keeps in Linger. **Item**: one sermon, photo (`handwritten`,
  `slide`, `page`) or `note`. **Source**: a book, preacher, church or website an item came from.
- **Chunk**: a passage of an item's text, the unit of search and citation. **Focus**: a chunk the user
  marked on a photo. **Anchor**: a link from an item or chunk to a verse range.
- **Verse id**: `book × 1,000,000 + chapter × 1,000 + verse` on the canonical 66-book grid (John 3:16 is
  `42_003_016`), the one encoding everywhere. **`uid`**: the client-made UUID every synced record
  carries. **`seq`**: the per-user sequence number every library write stamps, which orders the change
  feed. **`baseSeq`**: the `seq` a client's copy had when an edit began; a stale one makes a conflict
  copy instead of overwriting.
- **Digest**: the one-time model pass over a sermon or long page set that finds references, an outline
  and a summary.
- **Change feed / outbox / cache**: the sync engine's pull, push and local copy ([library.md](library.md#the-sync-engine)).
- **Unreviewed**: a photo Linger has read but the user hasn't checked; Ask can use it and says so.
- **Free / Pro**: the two plans; every allowance is in [backend.md](backend.md#plans-and-limits).
- **Zero retention**: every model call is zero-data-retention and never used for training
  ([backend.md](backend.md#privacy-and-consent)).

## Keeping the plans honest

- When a milestone lands, update its status line, [What's built](backend.md#whats-built) and anything
  the work proved different from the plan.
- A change to a design decision is made in the owning document first; milestone plans link to it
  rather than copying it.
- Numbers that come from tests (Convex behaviour, AVPlayer seeking, costs) say when and how they were
  measured.
