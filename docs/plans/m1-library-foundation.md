# M1: The library foundation

Status: not started. Depends on: nothing. Design: [library.md](library.md), [backend.md](backend.md).

**Goal.** Move the desktop app's personal data into Convex and keep it in sync, without changing how
reading feels. After M1, notes, highlights, bookmarks, Recents and recordings live in the library, sync
between two Macs (or a Mac and a Linux machine) within a second, work offline, and sermons are stored
and transcribed entirely in Convex. Vercel Blob is gone.

**Not in M1:** search and the new Ask tools (M2), iOS (M3), photos (M4), on-device transcription (M5),
reading position, plan and profile sync (M6).

## Before any code ships

These gate M1's release, because they can't be fixed once users have libraries:

1. **The production Clerk instance and `linger.app`.** Register `linger.app` and create the production
   instance for the "Linger" application on `auth.linger.app`, with Apple, Google and email. A new
   instance gives every user a new `sub`, so it must exist before the first library record is written
   ([backend.md](backend.md#clerk)). Remap the few development-instance accounts by email with an admin
   script. Update `CLERK_URL` and `CLIENT_ID` (`crates/core/src/auth.rs`), the proxy's `CLERK_*`
   variables and Convex's `CLERK_JWT_ISSUER`. Activate Clerk's Convex integration.
2. **AssemblyAI's written no-training commitment**, or a move to a transcription provider that gives one
   ([backend.md](backend.md#privacy-and-consent)).
3. **Two spikes, two days each:**
   - **Tokens:** sign in on the Mac app and on a phone (Clerk's hosted page is enough before M3) against
     a dev deployment, call a `whoami` query with each token through the `convex` crate, and confirm
     one `tokenIdentifier`. Check that `set_auth_callback` refetches on reconnect and that `set_auth`
     can swap a token before it expires.
   - **Remux:** a Node action that joins three ADTS parts and writes an M4A that AVPlayer reports at its
     exact duration, with a pure-JS muxer (`mp4-muxer` or `mediabunny`). If none works, take the
     fallback in [library.md](library.md#sermons).

## Convex

1. **Auth.** `convex/auth.config.ts` with the one `customJwt` provider
   ([backend.md](backend.md#convex-auth)). `convex/lib/user.ts`: `userQuery`, `userMutation` and
   `userAction` (convex-helpers custom functions) that read `ctx.auth.getUserIdentity()`, refuse a
   `subject` that isn't `user_…`, and find or create the `users` row (`ensureUser`, shared with the
   proxy's path); `canRead(ctx, doc)` and `canWrite`.
2. **Schema** ([library.md](library.md#the-data-model)):
   - synced: `items`, `highlights`, `bookmarks`, `chats`, each with `uid`, `seq`, `deletedAt?` and
     `by_user_seq`;
   - server-only: `anchors`, `files`, `userLibrary`;
   - `sermons` gains `itemId` and `audio` and loses `blobPath`;
   - `plans` gains `storageBytes` and `keepAudio`, seeded per [Plans and limits](backend.md#plans-and-limits).
3. **Every library write** goes through one helper that reads `userLibrary.seq`, gives each record it
   writes the next number, and keeps `userLibrary.storageBytes` current when files are attached or
   deleted.
4. **The canonical grid.** `cargo xtask contract` also writes `server/convex/lib/canon.json` (verses per
   chapter). `convex/lib/verses.ts` validates verse ids and ranges and splits a range into per-chapter
   index rows. A Rust test checks the JSON is current, like the contract test.
5. **Writes**, all idempotent by `uid` (or verse):
   - `library/notes.upsert({ uid, text, anchors })`, `library/notes.remove({ uid })`, which also rebuild
     the item's `anchors` index rows;
   - `library/highlights.set({ verse, color | null })`, `library/bookmarks.set({ verse, label | null })`;
   - `library/chats.save({ uid, title, fileUid })`, `library/chats.remove({ uid })`: the conversation JSON
     is uploaded and attached first as a `chat` file owned by the chat, so it counts toward storage;
   - `library/items.updateDetails({ uid, title?, details?, sermonUid?, offsetMs? })`.
6. **The change feed:** `library/sync.changes({ after })` returns every synced record with `seq > after`,
   tombstones included, in `seq` order, up to 500 records or 2 MB, with the last `seq` and the server's
   time, or `resync` when `after` is below `userLibrary.purgedThrough`. Records are in the client's shape
   (items with their `anchors` and `files` arrays, file `url`s, `uid`s, no `_id`s).
7. **Files:** `library/files.uploadUrl()` and `library/files.attach({ uid, ownerUid, role, storageId,
   partIndex?, page? })`, which checks the storage limit against the counter, refuses with `too_large`,
   and records the file's permanent `url`.
8. **Sermons in Convex** ([library.md](library.md#sermons)):
   - `library/sermons.begin({ uid, details, recordedOn })` creates the item as `recording` and the job,
     and answers `{ uploadAudio, cloudTranscript }` from the plan and allowance.
   - Audio parts arrive through `files.attach` with `role: audioPart`; Linux's FLAC arrives as
     `uploadPart` slices.
   - `library/sermons.finish({ uid, durationMs, partCount })` moves the item to `uploading` until every
     part is attached. Without `cloudTranscript` the item becomes `waiting` (for on-device transcription,
     or "Transcribe now" once Pro hours are available). Otherwise it reserves the duration (Pro hours or
     the Free trial hour, as `sermons.create` does today) and starts the workflow: join (ADTS parts:
     concatenate and remux to M4A; FLAC slices: concatenate) in a Node action, start AssemblyAI, wait for
     the webhook, store the transcript file, settle the real duration, delete AssemblyAI's job, mark the
     item `ready`, and delete a Free trial sermon's audio.
   - `library/sermons.transcribeNow({ uid })` starts the same workflow for a `waiting` Pro sermon once
     hours are available.
   - A cron finishes recordings with no new part for two hours and no `finish`, with the parts they have.
   - `convex/http.ts`: `POST /assemblyai` checks the per-job secret header against its stored hash and
     resumes the workflow.
   - The hourly cron checks on jobs whose webhook is late and fails jobs stuck over 3 hours, giving back
     the trial hour as today.
9. **`/v1/me`** gains `library: { storageUsed, storageLimit }` ([backend.md](backend.md#get-v1me)).
10. **Purge and tombstones.** Replace `account.purge` with the scheduled chain (`library/purge.step`, 500
    records a run) over every user table and file, ending with the `users` row
    ([backend.md](backend.md#deletion)). A daily cron hard-deletes tombstones older than 30 days with their
    files and index rows.
11. **Remove from the proxy:** `services/Blob.ts`, `services/Transcriber.ts`, `routes/DevBlob.ts`,
    `SermonFlow.ts`, the `/v1/sermons` handlers (the routes answer `409` for one release; see Risks),
    `/v1/webhooks/assemblyai`, `/v1/internal/cleanup`, the `sermons.expire` cron, and the Blob and
    AssemblyAI variables. `DELETE /v1/account` and the Clerk webhook start the purge chain. Disconnect
    the `linger-sermons` Blob store after the deploy.

## linger-core

1. **`study.rs` migrations.** `PRAGMA user_version` and numbered migrations. Migration 1 adds:
   - `cache_items` (with `anchors` and `files` as JSON), `cache_anchor_index` (per chapter, built from
     items), `cache_files` (a local path once downloaded), `sync_state` (the cursor and clock offset)
     and `outbox` (`id`, `kind`, `payload`, `attempts`, `last_error`, `created`);
   - `updated` and `deleted` on `highlights` and `bookmarks`; `uid` on `chats` (filled for existing rows).

   `notes` stays readable for migration and export, then becomes a view over note items. `StudyStore`'s
   public API keeps its shape where it can (`note(verse)`, `set_note`, `set_highlight`, `chats()`),
   reading the cache and writing through the outbox, so most of the desktop doesn't change.
   `generation` still bumps on every change, including ones from the feed. Verse keys convert between
   the packed `VerseRef` key and library verse ids at the sync boundary.
2. **`sync.rs`** (feature `sync`; depends on `convex` 0.10 and `tokio` with the current-thread runtime),
   as designed in [library.md](library.md#the-sync-engine):
   - `Sync::start(data_dir, convex_url, tokens: Arc<dyn TokenSource>) -> SyncHandle`, on a `"sync"`
     thread spawned **after the first frame**, so start-up stays at today's numbers;
   - the change-feed loop (apply a page in one SQLite transaction, then advance the cursor), the outbox
     drain with backoff, file uploads (`ureq` POST to the upload URL, then `files.attach`), token
     refresh before `exp`, and the server clock offset;
   - `SyncHandle::status()` (`Synced`, `Syncing(n)`, `Offline(n waiting)`, `SignedOut`, `Error`) and an
     `on_change` callback the desktop turns into `ctx.request_repaint()`.
3. **`reference::find_all(text) -> Vec<(Range<usize>, Reference)>`**, moved from
   `apps/desktop/src/pages/notes.rs` `mentioned()` and extended to whole chapters and ranges, with the
   existing tests.
4. **Recording** ([library.md](library.md#sermons)):
   - macOS: one continuous AudioToolbox `AudioConverter` (16 kHz mono AAC-LC) fed from the recorder
     thread, writing ADTS-framed packets and starting a new part every 5 minutes at a packet boundary, in
     `apps/desktop/src/recorder/`. WAV chunks continue as now.
   - Linux: one FLAC with a seek table at the end through `flacenc`, in core, uploaded in 8 MB slices.
   - Parts go to the outbox as they close, only if `begin` said `uploadAudio`; `finish` follows the last.
5. **`transcribe.rs`.** The proxy backend is replaced by the Convex flow. The transcript arrives through
   the change feed as a file and is cached as `transcript.json` beside the recording, so
   `transcribe::all` and the details sheet keep working. The env-key AssemblyAI backend stays for
   development.
6. **`api.rs`** gains `CONVEX_URL` (overridable with `LINGER_CONVEX_URL`) beside `API_URL`.

## The desktop app

1. **Start sync after sign-in** from `Account`. **Sign-out** waits for the outbox (or asks), then deletes
   the cache and cached files, keeping recordings that exist only on this Mac unless the user deletes them
   ([library.md](library.md#the-sync-engine)). The desktop already requires
   sign-in before reading (`sign_in_required`).
2. **Sync status** in the sidebar footer: a quiet "Synced", "3 changes waiting" when offline, and the error
   with Retry when something is refused.
3. **Notes** (`note_popover.rs`, `pages/notes.rs`): the popover edits the verse's own note item; the notes
   page lists note items, and a note carries every anchor `find_all` finds. "Edited on another device"
   when the feed replaces the open note.
4. **Recents** (`sidebar.rs`, `ask_panel.rs`): `uid`s instead of row ids; a conversation is saved as a
   file through the outbox and downloaded when opened on another device.
5. **Recordings:** the details sheet edits the sermon item; transcription status comes from the item
   (`recording`, `uploading`, `waiting`, `processing`, `ready`, `error`) instead of the polling thread. A
   `waiting` sermon says why: on-device transcription arrives in M5, and Pro transcribes in the cloud
   (with "Transcribe now" when hours are available).
6. **Migration** ([library.md](library.md#migrating-whats-on-the-device)): a one-time sheet ("Bringing
   your notes and sermons into your library") with progress, then the library consent sheet
   ([backend.md](backend.md#privacy-and-consent)). The Ask consent text drops "conversations stay on the
   device".
7. **The CLI** (`cli.rs`) reads notes and sermons from the cache.

## Tests

- **Convex (convex-test):**
  - another user's records are refused; the feed pages by `seq`, includes tombstones and never returns
    `_id`s;
  - **ordering:** two concurrent writes for one user get distinct, commit-ordered `seq`s, and a reader
    holding a cursor between them receives both; a page that ends partway through one mutation's records
    skips nothing; a cursor below `purgedThrough` gets `resync`;
  - writes are idempotent by `uid`; anchors split across chapter breaks, and invalid references are
    refused;
  - the storage counter matches the files after attach, delete and purge, and the limit holds;
  - `begin` answers `uploadAudio: false` for a Free user past the trial;
  - the purge chain removes every row and file for a user with 2,000 records and leaves another user's;
  - the sermon workflow settles the real duration, gives back the trial hour on failure, deletes trial
    audio, and resumes from a late webhook through the cron.
- **Core:** migrations from a copy of a real `study.sqlite`; the outbox survives a restart and drains in
  order; a feed page that replaces a record bumps `generation`; `find_all` on prose with chapters, ranges,
  ordinals ("1 John 4") and false friends ("Job 3 times", "Mark said"); ADTS parts from the encoder join
  to the recording's exact sample count.
- **Desktop:** `tests/ask_recents.rs` and `tests/empty_state.rs` move to the cache; a new test runs two
  `StudyStore`s against one local Convex backend and sees a note written in one appear in the other.
- **Sync load:** 1,000 clients subscribed to the feed while 100 write, against a dev deployment.
- **Start-up:** CI's startup probe stays within 5% of today's numbers with sync on.

## Done when

- A note written on one Mac appears on another within a second, and survives editing on both while one
  is offline (last write wins, and the open editor says it changed).
- An existing user's notes, highlights, bookmarks, Recents and recordings migrate once, with nothing lost
  or duplicated, and a second device with its own different notes keeps both sets.
- A one-hour sermon recorded on the Mac by a Pro user uploads as it records, is transcribed, plays back from
  Convex (checked with `curl` range requests and AVPlayer), and the allowance moves by its real duration.
  A Free user's recording past the trial uploads no audio.
- Nothing is left in Vercel Blob, and the proxy has no Blob or AssemblyAI code.
- Reading, notes and Recents work with the network off, and start-up time hasn't moved.

## Risks

- **The `convex` crate on tokio** adds weight to an app whose brand is speed. Keep it on its own thread
  after the first frame, and measure binary size and start-up in CI.
- **The per-user sequence serializes a user's writes.** A person's own write rate is low, but workflows
  (transcription, later extraction) write too; keep their writes small and few.
- **One note per verse becomes many.** The popover and margin keep today's behaviour for the verse's own
  note; other items appear as library marks in M2.
- **Old desktop builds** call `/v1/sermons`. For one release those routes stay, answering
  `409 contract_unsupported`, so old builds say "Update Linger" instead of failing oddly; then they go.
