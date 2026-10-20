# The library

Status: agreed design, October 19, 2026. Built in milestones M1–M8 ([roadmap](README.md#roadmap)).

This document owns the **personal library**: its data model, sync and handoff, capture (sermons,
photos, notes), references, search, the library's Ask tools, outside sources and the web client.
Accounts, the Ask proxy, model routing, plans and limits, payments, privacy and environments are
in [backend.md](backend.md). Facts live in one document; the other links to them.

## What Linger is

Linger is a **personal Bible context manager**. Reading the Bible is the main screen. Around it,
the user builds a library from what they study:

1. Sermons recorded on iOS or macOS.
2. Photos of handwritten notes.
3. Photos of sermon slides, taken while a sermon is being recorded or not.
4. Photos of book and commentary pages, circled and underlined with a finger to mark what matters.
5. Notes written in the app while reading.
6. Trusted outside sources: the lexicons, cross-references and people and places Linger already
   bundles, plus public-domain commentaries, reference works and confessions.

When the user asks a question while reading, Ask answers from that library and the Bible, and says
which item each claim came from. Everything is available on every device: photograph a page with
the iPhone while reading on the Mac, and it's on the Mac within seconds and usable by Ask as soon as
it has been read, usually within 30 seconds.

The library is **personal**. Nothing is shared between users. The schema leaves room for a church
library later (see [Later](#later)).

## Decisions

| Decision | Choice | Why |
|---|---|---|
| Where the library lives | **Convex**, for every personal record and every file (audio, photos, transcripts). No Vercel Blob | One store, one auth check, live queries for handoff; Convex file storage covers the files |
| How devices sync | Every client shares one **sync engine in `linger-core`**: a local cache, an outbox and a change feed from Convex, behind the **`LibraryBackend`** trait, the only door to Convex from a device | Offline reading and capture; one implementation for macOS, Linux, iOS and the web; a fake backend for tests |
| The schema | **Written once, in Rust.** `cargo xtask schema` generates the Convex validators, the Effect Schemas, the cache's columns and the FFI types, and a test fails on drift | Five copies of every record shape would drift; one can't |
| Where logic runs | **Rust in core, or Effect on the server** ([README](README.md#where-code-lives)). Swift, React and Node hold platform APIs and views | Every rule is reachable by a test through core or a layer |
| The unit | A **library item**: the original (audio, photo or none), its text, its **verse anchors** and where it came from | One shape for sermons, notes, slides and pages; verse anchors make "everything on Romans 8" an exact lookup |
| Retrieval | **Verse anchors and full-text search**, with vectors only if the M2 eval needs them | Anchors find what is about this passage; text finds what is about this idea, and the model rewrites its own queries |
| Photos to text | **Claude vision** through the AI Gateway, then a review screen | Reads handwriting and marked-up pages far better than OCR; the user corrects before it counts |
| Marks on photos | A **stroke layer** kept apart from the photo | Searchable "focus" text, re-extraction later, and the clean page stays available |
| Library vendor calls | **Convex actions** call AssemblyAI and the Gateway | The files are in Convex already; the Vercel proxy keeps Ask and billing |
| Ask | The existing proxy and client-side tool loop; two new tools, `search_library` and `read_item` | Proven under load; the new tools are one more client-side call |
| Web | **Reader and Ask only**, signed in. No recording or capture | The most limited client |
| Sharing | None for now. Every read goes through one access helper | Adding a church library later changes one function |
| Model data | **Zero data retention and no training, for every model call** | [backend.md](backend.md#privacy-and-consent) |

```diagram
┌──────────────────────────┐  outbox: notes, photos,  ┌──────────────────────────────────────────┐
│ iPhone (SwiftUI)         │  audio parts             │ Convex                                   │
│ camera, scanner, pencil, │ ───────────────────────▶ │  library: items, chunks, anchors, files  │
│ recording, reader, Ask   │ ◀── change feed ──────── │  workflows ─▶ AssemblyAI (sermons)       │
└──────────────────────────┘                          │           ─▶ AI Gateway (photos, digest) │
┌──────────────────────────┐                          │  library.search: anchors + full text     │
│ Mac / Linux (egui)       │ ───────────────────────▶ │  billing state: users, usage, ledger     │
│ reader, recording,       │ ◀── change feed ──────── └──────────────────────────────────────────┘
│ Continuity Camera, Ask   │                                    ▲ library.search (Ask tool, run
└──────────────────────────┘                                    │ by the client with its token)
┌──────────────────────────┐
│ Web (reader + Ask)       │ ── Ask rounds ─▶ Vercel proxy ─▶ AI Gateway ─▶ Claude (ZDR)
└──────────────────────────┘
```

## The data model

Every library record has a `userId` and goes through one `canRead(ctx, doc)` / `canWrite` helper.
Nothing reads by id without it.

**Verse ids.** A verse is the integer `book × 1,000,000 + chapter × 1,000 + verse` on the canonical
66-book grid (`BookId` 0–65); John 3:16 is `42_003_016`. Ids sort in canon order, so a range is two
numbers, and they read as references in the Convex dashboard. It is the one encoding: core's `VerseId`
newtype (`books.rs`) serializes to it, the cache stores it (migration 1 rewrites today's packed
`book << 16 | chapter << 8 | verse` keys and text book codes), and nothing converts at a boundary.
`cargo xtask schema` exports the grid (verses per chapter) as `canon.json` for the Convex functions,
which check every reference against it.

**Record ids.** Every synced record has a `uid`, a UUID the client makes, so a note or photo created
offline has an id before it's uploaded and a retried upload never makes a second copy. Migrated
recordings keep their recording UUID as the item's `uid`, so `{{sermon <id> …}}` citations in old
Recents still resolve. Citations and the cache use `uid`s; Convex `_id`s never leave the server.

### One schema

Every synced record is a Rust type in `crates/core/src/records.rs` (serde, with `schemars` for its
JSON Schema). `cargo xtask schema` writes `server/shared/records.ts`, holding an Effect `Schema` and a
Convex validator per record, `server/convex/lib/canon.json`, the cache's column list, and core's
`ApiError` from the server's error list; the FFI and the wasm bindings expose the same Rust types. A
core test fails if any generated file differs from what the code produces, as the contract test does.
The tables below describe those types; the Rust is the source.

**Synced records and server-only rows.** The apps sync a small set of record types, each carrying
`uid`, `seq` (below) and `deletedAt?`. Rows the server derives (chunks) or uses for indexing and
accounting (the anchor index, file rows) stay on the server, and what the client needs from them is
embedded in the record it belongs to.

| Synced record | Fields | Notes |
|---|---|---|
| `items` | `uid`, `kind` (`sermon`, `handwritten`, `slide`, `page`, `note`), `title`, `status` (`recording`, `uploading`, `waiting`, `processing`, `review`, `ready`, `error`), `text?` (notes), `textFileUid?` and `excerpt?` (sermons and photos), `summary?`, `outline?`, `sourceUid?`, `sermonUid?` + `offsetMs?`, `recordedOn?` (`macos`, `ios`, `linux`), `details?` (title, speaker and church, from `recording::Details`; `startedAt`, `endedAt?`), `capturedAt`, `unreviewed`; **`anchors`** (`[{ from, to, origin, atMs?, page? }]`); **`files`** (`[{ uid, role, contentType, bytes, url, page?, durationMs? }]`); for photos, **`pages`** (`[{ page, useWholePage, focus, printedPage?, marks? }]`) | One document per item; the embedded arrays are what the cache needs. A note's `text` is inline, because the user edits it offline. A sermon's or photo's text is a **`text` file** (`role: text`, named by its SHA-256, never rewritten in place): the transcript, or the pages' `text` and `pageText` as JSON. The record carries `textFileUid` and a short `excerpt`, so retitling a sermon doesn't resend its transcript to every device; the cache fetches text files eagerly (they're small beside audio), so offline search covers them. Capped well under Convex's 1 MiB (strokes are simplified to at most 2,000 points a page) |
| `sources` | `uid`, `kind` (`book`, `preacher`, `church`, `website`), `title`, `author?`, `publisher?`, `year?` | |
| `highlights` | `verse`, `color`, `createdAt` | One per verse, as now; `uid` is derived from the verse. `createdAt` is the device's time on a first upload (so migrated history keeps its dates), else the server's; kept on edits, set again when a tombstone comes back |
| `bookmarks` | `verse`, `label`, `createdAt` | One per verse, as now; `createdAt` as for highlights |
| `chats` | `uid`, `title`, `fileUid`, `url`, `bytes`; the conversation is a **file** | A conversation can approach 1 MB, so it isn't stored in the document; the app downloads it from `url` when opened |
| `positions` | `uid` (the device), `verse`, `translation`, `at` | The last place read on each device, for handoff (M6) |
| `planProgress` | `uid` (the plan id), `started`, `finished?`, `reads` | Reading plans (M6) |
| `profile` | `uid` (always `"profile"`), `askTranslation`, `homeChurch` | The two settings that follow the user (M6); one per user |

| Server-only table | Fields | Indexes |
|---|---|---|
| `anchors` | `userId`, `itemId`, `book`, `chapter`, `from`, `to`, `origin`, `atMs?`, `page?`: one row per chapter an anchor touches, rebuilt from the item's `anchors` | `by_user_chapter` [`userId`, `book`, `chapter`], `by_item` |
| `chunks` | `userId`, `itemId`, `ordinal`, `text`, `startMs?` / `endMs?`, `page?`, `focus`, `unreviewed`, `embedding?` | search index `text` (filters `userId`), `by_item`; a vector index only if M2 adds one |
| `files` | `userId`, `ownerUid` (the item or chat it belongs to; none for an export), `uid`, `role` (`audio`, `audioPart`, `uploadPart`, `photo`, `photoMarked`, `thumbnail`, `text`, `chat`, `export`), `storageId`, `contentType`, `bytes`, `partIndex?`, `page?`, `sha256` | `by_owner`, `by_user_uid` |
| `userLibrary` | `userId`, `seq`, `purgedThrough` (the highest `seq` whose tombstone was hard-deleted), `storageBytes` | `by_user`: one per user (below) |

Every synced table is indexed `by_user_seq` [`userId`, `seq`]. A file's `url` is its permanent
`ctx.storage.getUrl` link ([Files](#files)), so a device downloads from the record it already has.

How the existing `sermons` table fits: it stays the **transcription job** (status, AssemblyAI id,
webhook secret, reservation, trial flag), gains `itemId` and `audio`, and loses `blobPath` and the
24-hour transcript expiry. The item is what the user sees; the job is what billing and the webhook see.

Why it's shaped this way:

- **Anchors live on the item, and the server indexes them per chapter.** A range across a chapter
  break becomes two index rows. An anchor points into its item by time (`atMs`) or page, not by chunk,
  because chunks are rebuilt whenever the text changes; search finds the chunk covering that point. The reader asks "what is anchored to this chapter", which one indexed
  query answers on the server and the cache answers locally from the items it holds.
- **Chunks are the unit of search and citation**, rebuilt whenever an item's text changes. A sermon is
  chunked by transcript paragraphs (about 200 words, with times). A page is chunked by paragraph, and
  marked passages are their own chunks with `focus: true`. A note is one chunk. Offline, core indexes
  the cached items' text and text files with the engine that searches the Bible (`search.rs`), so the
  tool still answers with ranked passages.
- **Notes become items.** Today a note is one row per verse in `study.sqlite`, with no id. After M1 a
  note is a `note` item anchored to a verse or a range, and a verse can have several. The reader's note
  popover edits "your note on this verse": the note item whose only user anchor is that verse, made on
  first save. Other items anchored there show as library marks.
- **Highlights and bookmarks stay keyed by verse**, one per verse as now, last writer wins.
- **Sources are reused.** The first page photographed from a book asks for its title and author. Later
  pages pick the book from a list, so "what does Stott say about this?" can filter by source.
- **Tombstones.** Deletes set `deletedAt`, so other devices drop the record. A daily cron hard-deletes
  tombstones older than 30 days, with their files and server-only rows, and raises
  `userLibrary.purgedThrough`. A device whose cursor is older than that (offline for over a month) is
  told to resync: it keeps its outbox, empties its cache and pulls the library again.
- **Documents stay small.** Transcripts, page text, conversations and exports are files; items hold text, the
  embedded arrays and no blobs.

### Ask conversations are not context

Recents sync, but Ask's answers aren't searched as library material: feeding the model's answers back
as grounding would compound its mistakes. "Save to library" on an answer makes it a `note` the user
chose to keep.

## Sync

### Auth

`convex/auth.config.ts` has one `customJwt` provider: the Clerk instance's issuer and JWKS, and no
`applicationID`. It accepts the iOS and web session tokens and the desktop's OAuth access tokens.
Both carry the same issuer and the Clerk user id as `sub`, so a person has one `tokenIdentifier` on
every device. Convex doesn't check `aud` without `applicationID` and doesn't reject the OAuth `at+jwt`
header (tested on Convex Cloud, October 2026). Details in [backend.md](backend.md#convex-auth).

Library functions use `userQuery`, `userMutation` and `userAction` helpers that look the user up from
`ctx.auth.getUserIdentity()` (`users.by_clerk`), creating them if needed. A user id never comes from an
argument. The proxy's `serverQuery` / `serverMutation` secret stays for proxy-only functions.

### The sync engine

`crates/core/src/sync.rs`, behind a `sync` cargo feature (on for the desktop app and `crates/ffi`, off
for the web's wasm build):

- **One door: `LibraryBackend`.** A trait in core (`sync::Backend`) with every call a device makes to
  Convex: subscribe to the change feed, run a named mutation, run `library.search` and `items.read`,
  get an upload URL, upload bytes. Three implementations: the `convex` crate natively (feature
  `convex`), the Convex JavaScript client through wasm-bindgen on the web, and an in-memory fake for
  tests. The sync engine, the outbox and the Ask tools all go through it, so there is one connection,
  one token path and one thing to fake.
- **Its own thread.** The `convex` Rust crate (0.10) is async and the app has no async runtime, so the
  engine runs a single-threaded tokio runtime on a `"sync"` thread that owns the backend. The rest of
  the app talks to it through channels, as it does with the `"ask"` and `"account"` threads, and
  repaints when a change arrives; the Ask tool loop sends `search_library` and `read_item` down the
  same channel and blocks on the reply, so there is no second Convex client. iOS reaches the same
  engine through `crates/ffi`, with a callback that hands SwiftUI each change.
- **Tokens.** The engine asks the existing `auth::TokenSource` for a token on every connect, and pushes
  a fresh one 30 seconds before the current one's `exp`: Clerk session tokens on iOS last about a
  minute, the desktop's OAuth access token a day. The crate's `set_auth_callback` refetches on
  every reconnect and `set_auth` swaps a token in place (confirmed in M1,
  [backend.md](backend.md#answered-in-m1)).
- **One ordering for every write: a per-user sequence.** Every library mutation reads the user's
  `userLibrary.seq` and gives each record it writes the next number, so no two records share a `seq`. Because they all write that one
  document, Convex serializes a user's library writes, so `seq` order is commit order. A timestamp
  cursor wouldn't be safe: a mutation's clock is fixed when it starts, so one that started earlier can
  commit after a reader has moved past its time, and its row would never be delivered.
- **Pull: one change feed.** `library/sync.changes({ after })` returns records of every type with
  `seq > after`, tombstones included, in `seq` order, up to 500 records or 2 MB, whichever comes first,
  with the highest `seq` returned and the server's time; or `resync` if `after` is below
  `purgedThrough` and above 0 (a device starting from 0 has no copies to delete, so it just pulls
  what is there). Because every record has its own `seq`, a page can end anywhere without skipping. The engine subscribes, applies a page to the cache in one SQLite
  transaction, stores the new cursor and subscribes again. Convex re-runs the query when any of those
  rows change, so a photo saved on the iPhone reaches the Mac within a second.
  A device that meets a record type newer than itself stops at that page with "Update Linger to
  keep syncing" and never skips it, so a release that adds a record type leaves older apps
  stopped, not silently missing records, until they update.
- **Push: the outbox.** Every local write goes to an `outbox` table first, then to an idempotent
  mutation keyed by `uid` (`notes.upsert`, `highlights.set`, `chats.save`, …). An entry leaves the
  outbox when its mutation succeeds. Files upload through upload URLs (below) before the mutation that
  references them.
- **Conflicts: `baseSeq` first, then last writer wins.** Every upsert carries the `seq` the client's
  copy had when the edit began. If the server's record has moved past it, the server doesn't overwrite:
  for a note it keeps both, writing the late edit as a new note item with `conflictOf`, and for a
  highlight, bookmark or chat it keeps the server's and answers `conflict`, which the outbox treats as
  done. Every write's answer (`WriteAnswer`, highlights and bookmarks included) carries the record that stood (and, for a note, the copy), and an
  `ok` carries the record as stored: always exactly what the feed sends at that `seq`, which the
  writing device stores, so its copy (times included) is every other device's. Its own note becomes
  the conflict copy. Clearing a record that is already clear (or absent) answers `ok` with its current `seq` and
  consumes no sequence number. One person edits their own notes, so this is rare, but a late offline edit silently replacing
  a newer one would be data loss. The notes and Recents pages say "edited on another device" when the
  cache replaces a record the user has open, and show a conflict copy beside the note it collided with.
- **Server time.** Times that cross devices (when a recording started, when a slide was taken) are
  stored in server time. The engine keeps the device's clock offset in its local `sync_state`, measured
  on each connect from `library/sync.now`, a mutation that writes nothing, corrected by half the
  round trip; changes wait for it, since they carry creation times. Clock in a query is the time of
  that query's last execution, and Convex serves the cached result until a write invalidates it,
  so the feed's `serverTime` is not a clock and nothing measures from it.
- **The cache** is `study.sqlite`, which gains versioned migrations (`PRAGMA user_version`) in M1: the
  synced records, a per-chapter anchor index built from them, the cursor and the outbox. Reading, the
  margin, the notes page, Recents and the library list work offline from it. File bytes are fetched on
  demand and kept in `<data>/files/` with a size cap (thumbnails always; photos and audio
  least-recently-used). Text files and conversations are fetched as soon as the feed names them:
  they're small, offline search reads text, and Recents opens offline.
- **Signing out** waits for the outbox to drain (or asks), then deletes the cache and cached files, so
  nothing personal from the library stays on a shared computer; signing in again pulls it back.
  Recordings that exist only on this device (a Free user's audio, a recording not yet uploaded) are
  not deleted: sign-out says so and offers to delete them, and they reappear when the same account signs
  in again.

### Files

- **Upload.** A mutation returns a Convex upload URL (valid one hour). The client POSTs the bytes and
  gets a `storageId`, then calls `files.attach`, which checks the storage limit against the running
  `userLibrary.storageBytes` counter (never a scan of `_storage`). A POST must finish within 2 minutes,
  so nothing larger than about 10 MB goes in one POST: photos are under 1 MB, audio goes in parts, and
  any larger file goes in 8 MB byte slices (`uploadPart`) that an action concatenates.
- **Download.** `ctx.storage.getUrl` links need no sign-in and don't expire until the file is deleted.
  They're unguessable, handed only to the owner's apps and to AssemblyAI, and never logged. They answer
  `Range` requests with `206` and the exact bytes (tested), so audio seeks without downloading the whole
  sermon. Suffix ranges (`bytes=-N`) returned a 500 in the same test, so playback tests cover the
  request patterns AVPlayer and `<audio>` actually make.

### Migrating what's on the device

On the first launch after M1 (and after M1b for recordings), signed in, the app uploads once,
through the outbox:
- notes, as `note` items anchored to their verse;
- highlights, bookmarks and Recents (chats get `uid`s);
- recordings: details and transcript first (the recording's UUID becomes the item's `uid`), so they're
  searchable right away; then, **for Pro users only**, the audio, encoded as AAC parts in the background.
  Free users' audio stays on the device.

Local data stays until the upload is confirmed. A second device signing in pulls the library first,
then uploads only what's new: a highlight or bookmark already on the server for that verse is skipped,
and a local note is skipped only if a note with identical text is already anchored to its verse.
Otherwise it uploads as another note, so nothing is lost.

## Handoff

- **Captures appear everywhere live.** A photo taken on the iPhone appears on the Mac within seconds as
  "Reading your photo…". Once read (usually within 30 seconds), Ask on the Mac can use it, marked as not
  yet reviewed, before anyone has reviewed it.
- **Reading position.** `positions` lets any device, the web included, offer "Continue Romans 8 from
  your iPhone". On Apple platforms an `NSUserActivity` also puts Linger in the Dock and App Switcher for
  Handoff.
- **Reading plans** sync through `planProgress`, so a day read on the iPhone is checked on the Mac.
- **Two settings follow the user**, the Ask translation and the home church, in the `profile` record.
  Everything else (theme, text size, layout) stays per device.
- **Continuity Camera on the Mac.** "Take Photo" and "Scan Documents" from the iPhone straight into the
  Mac app, through `NSMenuItem.importFromDeviceIdentifier` and an AppKit responder bridged with objc2
  and placed in winit's responder chain (alongside `macos_window.rs`). Drag-and-drop and a file picker
  work on macOS and Linux.

## Capture

### Sermons

**Where they're recorded.**
- **macOS** records today: 30-second 16 kHz WAV chunks from the microphone or system audio (a Core Audio
  process tap), with title, speaker and church in `recording::Details`.
- **iOS** records with `AVAudioEngine`, with background audio, a Live Activity showing elapsed time and
  interruption handling (calls, Siri).
- **Linux** records through PulseAudio, as now.

**Starting.** `sermons.begin` creates the item as `recording` and answers what this recording may use:
`{ uploadAudio, cloudTranscript }`. Pro users with hours left get both; Pro users without hours left
upload audio (kept), and the sermon waits (`waiting`) to be transcribed on the device or, with "Transcribe
now", once hours are available again; Free users upload audio only for their unused trial hour,
otherwise nothing, and the sermon waits for on-device transcription ([plans](backend.md#plans-and-limits)). The answer is cached, so a
recording started offline follows the last known plan and is reconciled at `finish`.

**Encoding and upload (Apple platforms).**
- **AAC-LC, mono, 16 kHz** (the rate the Mac already records at), at the encoder's speech bitrate
  (32–48 kbps, at most about 22 MB an hour).
- **One encoder, in core, on both platforms.** AudioToolbox's `AudioConverter` is the same C API on
  macOS and iOS, so `recording::aac` (`crates/core`, feature `apple-audio`) calls it from Rust through
  `objc2-audio-toolbox` (or `coreaudio-sys`; the fallback is a C shim built by `build.rs`, never Swift).
  The platform hands core PCM buffers: the Mac's recorder thread as today, and on iOS the
  `AVAudioEngine` tap in Swift, which does nothing else. One continuous encoder per recording, because
  separate encoders per part would add priming silence at every join.
- **Framing and parts, in core.** `recording::ChunkWriter` generalizes to write the 7-byte ADTS header
  on each packet, start a new part file every 5 minutes at a packet boundary, and keep every packet's
  size and position for the muxer below.
- **Each part uploads as soon as it closes** (about 1.8 MB), through the outbox, as an `audioPart` file
  with its index. A two-hour service would be about 44 MB in one POST, which needs 3 Mbit/s of upload,
  more than church Wi-Fi or a phone signal reliably gives. Uploading as it goes also means most of a
  sermon is safe if the phone dies.
- The Mac keeps writing its WAV chunks too, for crash safety and on-device transcription.

**Encoding and upload (Linux).** There's no AAC encoder in the Linux stack, so Linux encodes one FLAC at
the end with the pure-Rust `flacenc` (about 55 MB an hour, with a seek table) and uploads it in 8 MB
`uploadPart` slices. `files.join`, the same action that joins M4A slices (a Node action for its
512 MiB of memory, enough for a two-hour file; a byte copy, no muxer), concatenates them. AssemblyAI,
AVPlayer and browsers all play FLAC.

**The M4A is written on the device.** The device holds every packet, so when recording ends core's
muxer (`recording::mp4`, a few hundred lines: one AAC track, `moov` with `stsz`, `stts`, `stsc` and
`stco` first, then `mdat`) writes one M4A from the packets it kept, without re-encoding, and uploads it
in 8 MB `uploadPart` slices; `sermons.finish` names it, `files.join` (a Convex action) concatenates
the slices byte for byte, and the parts are deleted once the M4A is attached. Why M4A: ADTS has no
index, so players estimate its timeline from the bitrate. In a test, AVPlayer gave a 259-second ADTS
file a duration of 238 seconds, 8% short; an hour into a sermon a transcript timestamp would land
minutes from its sentence. The same audio as M4A reported its exact duration and seeked exactly. Only
a recording whose device died has no M4A: the cron that finishes it concatenates the ADTS parts it has
into the `audio` file, and that sermon's timeline is approximate, which the item says.

**Transcription** is a `@convex-dev/workflow` workflow (Convex actions aren't retried on their own; the
workflow retries each step):
1. Reserve the duration against the plan, as `sermons.create` does today.
2. Start an AssemblyAI job on the joined file's `getUrl` link, with the settings `transcribe.rs` and
   `Transcriber.ts` use today: Universal-3.5 Pro, speaker labels and the 66 book names as key terms.
3. On AssemblyAI's webhook, a Convex HTTP action at `https://<deployment>.convex.site/assemblyai`
   checks the per-job secret, stores the transcript as the item's `text` file (`{ language, durationMs, segments }`, the
   shape the app already reads), settles the real duration and deletes AssemblyAI's copy. A cron
   checks on jobs whose webhook is late, as `GET /v1/sermons/:id` does today.
4. Run the **digest step** (below), then chunk and anchor the transcript.

Pro audio is **kept**, so a recording plays on any device and a transcript line jumps to its moment. A
Free trial sermon's audio is deleted once transcribed.

**On-device transcription** (Free, offline, or by choice):
- **iOS 26 and macOS 26:** Apple's SpeechTranscriber, through one small Swift static library with a C
  ABI on both platforms. It takes an audio file and the contextual strings and returns the transcript
  shape below, nothing else: the one Swift leaf with no Rust equivalent, because the API is Swift-only.
- **macOS 11–15 and Linux:** `whisper-rs` with the `base.en` model, downloaded on first use.
- **iOS 17–18:** none; these users have the cloud trial hour, or Pro.

The transcript uploads as a file and goes through the same digest, chunking and anchoring. On-device
transcripts are lower quality, and the item says so.

**Slides and sermons.**
- **During a recording on the same phone,** a photo attaches to that sermon with `offsetMs` from the
  recording's start.
- **During a recording on another device** (the Mac on the lectern, say), the capture screen offers to
  attach to it: a sermon in `recording` status that started within the last four hours is visible to all
  the user's devices through the change feed, and both times are in server time, so `offsetMs` is the
  photo's time minus the recording's start. A sermon that has stopped (`uploading` or later) is no
  longer offered.
- **A recording whose device died** (no new part for two hours, no `finish`) is finished by a cron with
  the parts it has, so what was uploaded isn't lost.
- **Afterwards,** any slide can be attached to a sermon, or moved, from the item's page
  (`items.updateDetails` sets `sermonUid` and `offsetMs`).
- **Without a recording,** "Sermon notes" makes a sermon item with no audio (title, speaker, church,
  date) for slides and handwritten notes from one service to gather under.

`read_item` on a sermon returns its slides inline at their times, so Ask sees what was on screen while
each point was made.

### Photos

1. **Capture.**
   - iOS: the camera for slides and notes; the document scanner (`VNDocumentCameraViewController`) for
     book pages, which straightens and crops them. **A scan of several pages is one item** with one
     `photo` per page (`page` 1, 2, …).
   - The user picks a kind: handwritten notes, slide or book page. Extraction checks the kind and can
     suggest another.
   - macOS: Continuity Camera, drag-and-drop or a file picker. Linux: drag-and-drop or a file.
2. **Mark (optional).** On iOS, a PencilKit canvas over each page: circle, underline or highlight with a
   finger or Pencil. Each stroke's raw points, pen kind and colour cross the FFI as they are; core
   converts them to Linger's format (normalized 0–1 points), simplifies them and saves them apart from
   the image. Swift draws the canvas and nothing more. The Mac gets the same tools in M6, on the same
   converter.
3. **Render and upload.** Each page as JPEG at 2,048 px on the long edge (about 400 KB) and a
   thumbnail, with its marks in the item's `pages` entry; for a marked page, also `photoMarked`, drawn by **one shared renderer** in `linger-core`
   (`tiny-skia`, reached from Swift through the FFI), so every device draws the same marks the same way.
   `photos.begin` creates the item as `uploading`; the files attach through the outbox; `photos.submit`
   starts extraction. That sequence is core's (`photos::Capture`), driven through the outbox, so no
   platform can order it differently. Rendering on the device keeps image processing out of Convex.
4. **Extract.** A workflow step per page sends Claude **the clean photo and, for a marked page, the
   marked render**, so highlighter strokes don't hide the words, and asks for structured output:
   - `text`: the whole page or slide, in reading order;
   - `focus`: each marked passage, with how it was marked;
   - `references`: every Bible reference, normalized, including "v. 12" resolved from the page;
   - `title`, and for pages the running head and printed page number.

   The model per kind is a row in the `models` table (`extract-handwriting`, `extract-print`). The M4
   eval decides the starting rows: Sonnet 5.5 for handwriting, and Haiku 5.5 for slides and printed
   pages if it reads them as well. The item becomes `review`, with the proposal chunked provisionally
   (`unreviewed`), so Ask can already use it, saying it hasn't been reviewed.
5. **Review.** The user sees each page beside its extracted text with the focus passages highlighted,
   corrects the text, confirms or edits the references, and picks or adds the book. Saving makes it
   `ready` and re-chunks it as reviewed. A setting skips review for slides.

**What's stored from a marked page is what was marked**, page by page. For a marked page, the page's
`text` is the marked passages, each with the sentence before and after for context, and only that is
chunked, searched and given to Ask. The whole page's extracted text (`pageText`) is shown during review;
unless the user turns on "Use the whole page" for that page, it's discarded when the review is saved.
Both live in the item's `text` file, which approval rewrites.
Unmarked handwriting and slides store their full text; an unmarked book page stores its full text too,
and review suggests marking it.

The original photos are kept for the user, so an item can be re-extracted when models improve; a photo
is an image the user owns, not text Linger searches or sends to Ask. Citations
name a page by its position in the item (`p2`); the printed page number is shown beside it.

### Notes in the app

The note popover and the notes page write `note` items, anchored to the selected verse or range. A note
can also be written without a verse, from the library. References typed in a note ("see Rom 5:8") are
anchored too, through `reference::find_all(text)`. It generalizes `mentioned()` in
`apps/desktop/src/pages/notes.rs` (which finds `Book c:v` by trying up to three words before each
`c:v`) into core, so every client anchors notes the same way, and adds whole chapters ("Romans 8") and
ranges.

### References: parsed, then checked

References come from three places, recorded in each anchor's `origin`:
- the user (the verse a note is attached to, or the references confirmed on review);
- `reference::find_all` over text typed in the apps;
- the model, from photos and the digest step.

Every reference is checked against the canonical grid before it's stored: a book, chapter and verse that
exist, from ≤ to. One that fails is dropped and logged, never guessed.

### The digest step

After a sermon is transcribed, or a page set of three or more pages is saved, a model reads it once (the
`digest` row in `models`, Haiku 5.5 to start) and returns:
- the references the preacher read or cited, or the pages cite, each with its transcript time or page;
- a short outline;
- a one-paragraph summary.

The summary and outline go on the item, and each reference becomes an anchor at that time or page. This is what makes "what did I hear about Romans 8?" find a sermon whose explanation never says
"Romans" in the same paragraph. It replaces the guess `transcribe::main_passage` makes today.

## Search and Ask

### `library.search`

A Convex action taking `{ query, passage?, kinds?, source?, limit }`. It searches reviewed and
unreviewed chunks alike and marks the unreviewed ones.
0. **Source.** `source` is a name, matched case-insensitively against the user's sources (title and
   author) and sermons' speaker and church, so "Stott" finds a book and "Pastor Lee" finds sermons.
1. **Anchors.** If there's a passage, the chunks anchored to any verse in it, through
   `by_user_chapter`.
2. **Text.** The full-text index over chunks, filtered to the user. Convex's text search lowercases
   Latin text, matches only the last word as a prefix and has no typo tolerance; the Ask model writes
   the queries and can try several phrasings.
3. **Vectors, only if added** (below).
4. **Merge** with reciprocal-rank fusion. Focus chunks and anchored chunks get a boost. Results are
   grouped by item, so one sermon doesn't fill every slot.

It is the `search` action in `convex/library/search.ts`. It returns chunks with their item's `uid`,
kind, title, source, date, times and page, and whether they're unreviewed. It is an action, not a
query, because it merges several reads, so it is neither reactive nor cached; counts like "7 items on
Romans 8" come from the anchor query instead. Every client calls it through `LibraryBackend`
([the sync engine](#the-sync-engine)).

### Vectors, if the eval asks for them

M2 starts with anchors and text and measures citation recall on its eval. Vectors would add matches
by meaning ("grief" finding a sermon about lament), but Convex bills each vector search for the
**whole index**, whatever the filter, and every user's chunks share one index. At 1,000 Pro users
with 1,500 chunks each, 512-dimension vectors make a 3 GB index, so each search costs 3 query-GB:
about 16,000 searches a month inside the Professional plan's 50,000. Add vectors only if the eval
shows a meaningful gain, at 256 or 512 dimensions, with the embedding model as a `models` row. Revisit
if the cost grows with users.

### The library tools (contract `ask/5`)

- **`search_library { query, passage?, kinds?, source?, limit? }`** calls `library.search`; `source` is a
  book, author, preacher or church name. Its description says the library is
  what this user has heard, read and written: not Scripture and not necessarily right, and that
  unreviewed items may have reading mistakes.
- **`read_item`** returns an item's text or a window of it: a sermon around a time with its slides
  inline, or a page with its focus passages marked.
- They replace `search_sermons` and `read_sermon`, since sermons are items now. `chat::Tools` loses its
  `sermons` field, so the desktop stops reading every transcript from disk on the UI thread before
  each question.
- `get_passage`, `search_bible`, `original_words`, `lexicon`, `word_occurrences`, `cross_references`
  and `people_and_places` don't change and still run on the device.

The proxy never sees library contents except as tool results inside the conversation it already
forwards. How contract versions are served is in [backend.md](backend.md#the-ask-contract).

### The prompt and citations

The system prompt gains three rules:
1. **Use the user's library when it bears on the question**, and say so by name: "In Pastor Lee's
   sermon on March 3…", "Your notes on this chapter say…".
2. **Keep the user's material apart from the text.** When a note, sermon or book says something the
   passage doesn't support, say so gently and show the passage.
3. **Cite each library claim** with `{{item <uid prefix> [mm:ss | p<page>]}}`. This generalizes
   today's `{{sermon <id> mm:ss}}` marker, which old chats keep rendering. The Ask panel renders it as
   a chip that opens the item at that place: the photo with its marks, or the sermon at that second.

The request still names the passage being read. The Ask panel shows how much of the library touches
it ("7 items on Romans 8") before the user types, from the same anchor query the margin uses.

### Licensing of library material

- **Bible text sent to a model stays public domain**, by the gate in
  [backend.md](backend.md#licensing-shapes-the-product).
- **Library material is the user's own**, for private study. Photos of books may contain copyrighted
  text, including quoted modern translations. API.Bible's AI restriction covers text obtained from
  API.Bible, not a user's photo of a book they own. Personal, private use is the defensible case, so:
  - nothing in one user's library is ever visible to another, or to any shared index;
  - a marked page stores and searches only what was marked, with a sentence of context
    ([Photos](#photos)), so what reaches Ask is excerpts rather than whole books;
  - every model call is zero-retention and never used for training.
- Zero retention settles what vendors keep, not copyright. A lawyer confirms this reading for book
  photos and sermon audio before any public release of book-page capture (M4's release gate).

## Outside sources

Bundled today, compiled into the app: STEPBible's tagged Hebrew and Greek with Abbott-Smith, BDB and
Strong's (`original`), OpenBible.info cross-references (`crossrefs`) and people and places (`names`).

Added in M7, public domain, each recorded in `LICENSES/README.md`:
- **Commentaries:** Matthew Henry, Jamieson-Fausset-Brown, Barnes' Notes, Keil & Delitzsch, and
  Calvin's commentaries in their 19th-century translations.
- **Reference:** Easton's Bible Dictionary, Nave's Topical Bible.
- **Creeds, confessions and catechisms:** the Apostles', Nicene and Athanasian Creeds; the Westminster
  Confession and Catechisms; the Heidelberg Catechism; the Belgic Confession; the Canons of Dort; the
  1689 London Baptist Confession. Several traditions, so Linger doesn't pick one.

**Where they live.** The commentaries are tens of megabytes even compressed: too much to compile into
every app and too much for the web to download. They go in **shared, read-only Convex tables**
(`sourceWorks`, `sourceEntries`, and `sourceAnchors` indexed per chapter like the library's anchors),
built by `cargo xtask build-sources` and loaded with a deploy script. Keeping every outside source there keeps the tool simple.

**The tools** (contract `ask/6`): `commentaries(passage, works?)` returns what each commentary and
confession says on the passage, with the work's name, date and tradition; `topics(term)` returns
Easton's and Nave's entries and matching confession sections. The prompt asks the model to attribute
("Matthew Henry reads this as…"), never to present a commentary as the meaning of the text. Details
in [M7](m7-outside-sources.md).

## Web: reader and Ask

The web app reads the Bible and asks questions, signed in. It doesn't record or capture. It opens
items read-only, because Ask's citations must open somewhere, and lets the user edit notes,
highlights and bookmarks.

- **UI:** a TypeScript single-page app in `web/` (Vite and React), hosted on Vercel as its own
  project, with HTML and CSS typesetting in Literata, `ConvexProviderWithClerk` and Clerk's React
  SDK. React renders and holds no logic. egui on the web was rejected: no native text selection, find in page or screen-reader support,
  and a large download before first paint.
- **Core on WebAssembly.** The Bible, search, references, the Ask tool loop and the library's client
  side come from `linger-core` compiled to `wasm32` through a new `crates/web` (wasm-bindgen). That
  needs a `native` feature gating every native-only dependency (`rusqlite`, `ureq`, `keyring`, `hound`,
  `flacenc`, the zstd C library, audio playback, image handling, `whisper-rs`, the `convex` crate and
  the `apple-audio` encoder), with the pure-Rust `ruzstd` decoder for the data and getrandom's
  `wasm_js` backend for `uuid`. Today the data is compiled in with `include_bytes!`; the wasm build
  fetches it instead (about 7 MB of translations, 4 MB of original-language text, 1 MB of
  cross-references) on first use and keeps it in Cache Storage.
- **The tool loop is `assistant.rs` in wasm**, not a port. `assistant` and `api` reach the proxy through
  an `Http` trait: `ureq` natively, `fetch` under wasm, a fake in tests. Rounds go to the same proxy,
  which allows the web origin through CORS; the device-side tools run in wasm, and `search_library`
  goes through the web's `LibraryBackend`, which wraps the Convex JavaScript client, so Clerk's
  session token and Convex's live queries stay the JavaScript SDKs' job and only the view is React.
- **Handoff:** the web reads and writes `positions`, so it offers "Continue from your iPhone" and the
  apps offer to continue from the web.

The wasm build is the risky part, so M8 spikes it first. There is no TypeScript fallback: if the
binary is too big or too slow, the fix is in the build (lazy data, fewer features, more fetches), not
a second implementation ([README](README.md#where-code-lives)).

## What the library costs

Prices are Anthropic list prices through the Gateway and Convex's Professional plan.

| What | Unit cost |
|---|---|
| Photo, handwriting (Sonnet 5.5: ~1,600 input tokens per image, two images when marked, ~800 output) | ~$0.011, or ~$0.014 marked |
| Photo, slide or printed page (Haiku 5.5) | ~$0.0006 |
| Sermon digest (Haiku 5.5: ~12k in, ~1.5k out) | ~$0.002 |
| Transcription (AssemblyAI) | ~$0.26 an audio hour |
| Audio (AAC, 16 kHz mono) | at most ~22 MB an audio hour |
| Photo page with its marked render and thumbnail | ~0.9 MB |
| Convex file storage (100 GB included) | $0.03 per GB-month |
| Convex file egress: playback and AssemblyAI fetches (50 GB a month included) | $0.12 per GB |

A typical Pro user who records four sermons and takes forty photos a month adds about **$0.25** in
extraction and digests, and about **125 MB** of storage, about $0.05 a month after a year. Storage is
cheap; extraction volume and vector search (if added) are the costs to watch. The allowances are in
[backend.md](backend.md#plans-and-limits); extraction and digests are metered in the same `ledger`.

## Later

- **Church libraries.** A church records a sermon once and its members follow the church. Items gain
  an `ownerId` that is a user or a Clerk organization, and `canRead` admits members. The church holds
  the rights to its own sermons, which settles the recording question too.
- **Sharing one item** with a friend or small group, through the same helper.
- **Re-extraction** of old photos when a better model ships, as an opt-in.
- **Vectors**, if M2's eval didn't need them but usage later shows misses.
