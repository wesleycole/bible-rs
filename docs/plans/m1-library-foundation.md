# M1: The library foundation

Status: not started. Depends on: nothing. Design: [library.md](library.md), [backend.md](backend.md).

**Goal.** Move the desktop app's personal data into Convex and keep it in sync, without changing how
reading feels. After M1, notes, highlights, bookmarks and Recents live in the library, sync between two
Macs (or a Mac and a Linux machine) within a second, and work offline. The foundations every later
milestone builds on are in place: Effect inside Convex, the record schema generated from Rust, and one
`LibraryBackend` in core.

**Not in M1:** sermons on Convex (M1b), search and the new Ask tools (M2), iOS (M3), photos (M4),
on-device transcription (M5), reading position, plan and profile sync (M6).

## Before any code ships

These gate M1's release, because they can't be fixed once users have libraries:

1. **The production Clerk instance and `linger.app`.** Register `linger.app` and create the production
   instance for the "Linger" application on `auth.linger.app`, with Apple, Google and email. A new
   instance gives every user a new `sub`, so it must exist before the first library record is written
   ([backend.md](backend.md#clerk)). Remap the few development-instance accounts by email with an admin
   script. Update `CLERK_URL` and `CLIENT_ID` (`crates/core/src/auth.rs`), the proxy's `CLERK_*`
   variables and Convex's `CLERK_JWT_ISSUER`. Activate Clerk's Convex integration.
2. **Three spikes, two days each:**
   - **Tokens** (done, October 19): a desktop OAuth access token and a Clerk session token for the
     same user resolve to one `tokenIdentifier` through `library/whoami:get` and the `convex` crate;
     `set_auth_callback` refetches on reconnect, and `set_auth` swaps a token before it expires. A
     fixed token past `exp` leaves writes hanging, so the engine uses the callback and pushes a fresh
     token early ([backend.md](backend.md#answered-in-m1)).
   - **Effect in Convex** (done, October 19): `highlights.set` on `convex/lib/effect.ts` beside the
     same mutation written plainly (`library/spike.ts`). In convex-test the Effect handler runs at
     0.11–0.15 ms median against 0.08 ms plain, so the overhead is under a tenth of a millisecond a
     call; the anonymous local deployment bundles and serves the functions. On a cloud dev deployment
     (`adorable-porcupine-835`, October 19, 87 alternating calls each from `ConvexHttpClient` with a
     real OAuth token, timings from `npx convex logs --jsonl`): Convex's `userExecutionTime` for
     `highlights.set` is 24.8 ms median (p95 30.8 ms) against 24.1 ms (p95 33.0 ms) plain, and
     `executionTime` 33.9 ms median for both, so the overhead doesn't show at Convex's resolution and
     is far inside the 1 s budget. The whole deployment bundles to 694 KB (179 KB gzipped,
     `--debug-bundle-path`), of Convex's 32 MiB limit; Effect is one 524 KB chunk shared by every
     module. `spike.ts` is deleted. Effect is the decision; the spike sized it.
   - **The schema generator** (done, October 19): `cargo xtask schema` end to end for `Highlight` and
     `Bookmark`: the Rust types, the generated validator fields and Effect `Schema`s, `canon.json`,
     and the drift tests in `records.rs`. The change feed's `FeedRecord` (serde tag `type`) and
     `Changes` are Rust types too, emitted as Effect `Schema.Union`s and Convex `v.union`s. Still to
     add: the cache's column list, the FFI types, and core's `ApiError` from the server's error list.

## Convex

1. **Auth.** `convex/auth.config.ts` with the one `customJwt` provider
   ([backend.md](backend.md#convex-auth)). `convex/lib/user.ts`: `userQuery`, `userMutation` and
   `userAction` that read `ctx.auth.getUserIdentity()`, refuse a `subject` that isn't `user_…`, find or
   create the `users` row (`ensureUser`, shared with the proxy's path), and provide `CurrentUser`;
   `canRead(ctx, doc)` and `canWrite`.
2. **Effect in handlers** ([backend.md](backend.md#effect-in-convex)): `convex/lib/effect.ts` with
   `ConvexCtx`, `CurrentUser`, `ServerCaller`, `effectQuery` / `effectMutation` / `effectAction`, and
   the mapping from `shared/Errors.ts` to `ConvexError`. `Errors.ts` moves from `src/` to `shared/` so
   the proxy and `convex/` import one list. Today's billing functions (`usage.ts`, `entitlements.ts`,
   `account.ts`, `webhooks.ts`, `sermons.ts`) move onto it behind their existing tests, so the
   `Allowance` service exists before the library needs it.
3. **The generated schema.** `crates/core/src/records.rs` defines every synced record and `VerseId`;
   `cargo xtask schema` writes `server/shared/records.ts` (an Effect `Schema` and a Convex validator
   per record), `server/convex/lib/canon.json` (verses per chapter), the cache's column list and core's
   `ApiError`. `schema.ts` composes the generated validators with the indexes. A core test fails on
   drift ([library.md](library.md#one-schema)).
4. **Schema** ([library.md](library.md#the-data-model)):
   - synced: `items`, `highlights`, `bookmarks`, `chats`, each with `uid`, `seq`, `deletedAt?` and
     `by_user_seq`;
   - server-only: `anchors`, `files`, `userLibrary`;
   - `plans` gains `storageBytes` and `keepAudio`, seeded per [Plans and limits](backend.md#plans-and-limits).
5. **Services**, each a `Context.Service` over `ConvexCtx` with a test layer: `Sequence` (reads
   `userLibrary.seq`, numbers each record a mutation writes, keeps `storageBytes` current when files
   are attached or deleted), `Allowance` (every plan check, count and reservation; `beginRound` moves
   onto it), `Files`, `Anchors`. `convex/lib/verses.ts` is a pure module over `canon.json`: validate
   ids and ranges, split a range into per-chapter index rows. `convex/lib/conflict.ts` is a pure module
   that decides what a stale `baseSeq` does per record kind.
6. **Writes**, all idempotent by `uid` (or verse), each carrying `baseSeq`
   ([library.md](library.md#the-sync-engine)):
   - `library/notes.upsert({ uid, text, anchors, baseSeq })`, `library/notes.remove({ uid })`, which also
     rebuild the item's `anchors` index rows; a stale `baseSeq` writes the edit as a new note with
     `conflictOf`;
   - `library/highlights.set({ verse, color | null, baseSeq })`, `library/bookmarks.set({ verse, label | null, baseSeq })`;
     a stale `baseSeq` keeps the server's and answers `conflict`;
   - `library/chats.save({ uid, title, fileUid, baseSeq })`, `library/chats.remove({ uid })`: the
     conversation JSON is uploaded and attached first as a `chat` file owned by the chat, so it counts
     toward storage;
   - `library/items.updateDetails({ uid, title?, details?, sermonUid?, offsetMs?, baseSeq })`.
7. **The change feed:** `library/sync.changes({ after })` returns every synced record with `seq > after`,
   tombstones included, in `seq` order, up to 500 records or 2 MB, with the last `seq` and the server's
   time, or `resync` when `after` is below `userLibrary.purgedThrough`. Records are in the generated
   shape (items with their `anchors` and `files` arrays, file `url`s, `uid`s, no `_id`s).
8. **Files:** `library/files.uploadUrl()` and `library/files.attach({ uid, ownerUid, role, storageId,
   sha256, partIndex?, page? })`, which checks the storage limit through `Allowance`, refuses with
   `too_large`, and records the file's permanent `url`.
9. **`/v1/me`** gains `library: { storageUsed, storageLimit }` ([backend.md](backend.md#get-v1me)).
10. **Purge and tombstones.** Replace `account.purge` with the scheduled chain (`library/purge.step`, 500
    records a run) over every user table and file, ending with the `users` row
    ([backend.md](backend.md#deletion)). A daily cron hard-deletes tombstones older than 30 days with their
    files and index rows. `DELETE /v1/account` and the Clerk webhook start the chain.

## linger-core

1. **`records.rs`.** The synced record types, `VerseId`, `Anchor`, `FileRef`, the status enums, and the
   `schemars` derivations the generator reads. `books.rs` gains `VerseId` conversions from `VerseRef`
   for the reader's own use; the wire and the cache use `VerseId` only.
2. **`study.rs` migrations.** `PRAGMA user_version` and numbered migrations. Migration 1 adds:
   - `cache_items` (with `anchors` and `files` as JSON), `cache_anchor_index` (per chapter, built from
     items), `cache_files` (a local path once downloaded), `sync_state` (the cursor and clock offset)
     and `outbox` (`id`, `kind`, `payload`, `base_seq`, `attempts`, `last_error`, `created`);
   - `updated` and `deleted` on `highlights` and `bookmarks`; `uid` on `chats` (filled for existing
     rows); verse keys rewritten from the packed form to `VerseId`.

   `notes` stays readable for migration and export, then becomes a view over note items. `StudyStore`'s
   public API keeps its shape where it can (`note(verse)`, `set_note`, `set_highlight`, `chats()`),
   reading the cache and writing through the outbox, so most of the desktop doesn't change.
   `generation` still bumps on every change, including ones from the feed.
3. **`sync.rs`** (feature `sync`; the `convex` backend behind feature `convex`, which depends on
   `convex` 0.10 and `tokio` with the current-thread runtime), as designed in
   [library.md](library.md#the-sync-engine):
   - `sync::Backend`, the `LibraryBackend` trait: `subscribe_changes`, `mutate(name, args)`,
     `action(name, args)`, `query(name, args)`, `upload_url`, `upload(bytes)`; `ConvexBackend` and
     `FakeBackend` (in-memory, with a scriptable feed and failure injection);
   - `Sync::start(data_dir, backend: Box<dyn Backend>, tokens: Arc<dyn TokenSource>) -> SyncHandle`, on
     a `"sync"` thread spawned **after the first frame**, so start-up stays at today's numbers;
   - the change-feed loop (apply a page in one SQLite transaction, then advance the cursor), the outbox
     drain with backoff, file uploads (upload URL, bytes, then `files.attach`), token refresh before
     `exp`, and the server clock offset;
   - `SyncHandle::status()` (`Synced`, `Syncing(n)`, `Offline(n waiting)`, `SignedOut`, `Error`), an
     `on_change` callback the desktop turns into `ctx.request_repaint()`, and `SyncHandle::call` for
     requests other threads (the Ask tool loop, from M2) send through the backend.
4. **`reference::find_all(text) -> Vec<(Range<usize>, Reference)>`**, moved from
   `apps/desktop/src/pages/notes.rs` `mentioned()` and extended to whole chapters and ranges, with the
   existing tests.
5. **`api.rs`** gains `CONVEX_URL` (overridable with `LINGER_CONVEX_URL`) beside `API_URL`, and
   `ApiError` becomes the generated enum.

## The desktop app

1. **Start sync after sign-in** from `Account`. **Sign-out** waits for the outbox (or asks), then deletes
   the cache and cached files, keeping recordings that exist only on this Mac unless the user deletes them
   ([library.md](library.md#the-sync-engine)). The desktop already requires
   sign-in before reading (`sign_in_required`).
2. **Sync status** in the sidebar footer: a quiet "Synced", "3 changes waiting" when offline, and the error
   with Retry when something is refused.
3. **Notes** (`note_popover.rs`, `pages/notes.rs`): the popover edits the verse's own note item; the notes
   page lists note items, and a note carries every anchor `find_all` finds. "Edited on another device"
   when the feed replaces the open note, and a conflict copy shown beside the note it collided with.
4. **Recents** (`sidebar.rs`, `ask_panel.rs`): `uid`s instead of row ids; a conversation is saved as a
   file through the outbox and downloaded when opened on another device.
5. **Migration** ([library.md](library.md#migrating-whats-on-the-device)): a one-time sheet ("Bringing
   your notes into your library") with progress, then the library consent sheet
   ([backend.md](backend.md#privacy-and-consent)). Recordings follow in M1b, through the same sheet.
   The Ask consent text drops "conversations stay on the device".
6. **The CLI** (`cli.rs`) reads notes from the cache.

## Tests

- **Convex (convex-test, through the Effect test layers):**
  - another user's records are refused; the feed pages by `seq`, includes tombstones and never returns
    `_id`s;
  - **ordering:** two concurrent writes for one user get distinct, commit-ordered `seq`s, and a reader
    holding a cursor between them receives both; a page that ends partway through one mutation's records
    skips nothing; a cursor below `purgedThrough` gets `resync`;
  - writes are idempotent by `uid`; a stale `baseSeq` on a note makes a conflict copy and on a highlight
    keeps the server's; anchors split across chapter breaks, and invalid references are refused;
  - the storage counter matches the files after attach, delete and purge, and the limit holds;
  - the purge chain removes every row and file for a user with 2,000 records and leaves another user's;
  - every error a handler can fail with is a class in `shared/Errors.ts` (a type-level test).
- **Generated schema:** the drift test; a record added in Rust without regenerating fails CI; a value
  the Convex validator accepts is one the Effect `Schema` decodes, checked by property test over the
  generated pairs.
- **Core:** migrations from a copy of a real `study.sqlite`, including the verse-key rewrite; the
  outbox survives a restart and drains in order against `FakeBackend`; a feed page that replaces a
  record bumps `generation`; a conflict answer removes the outbox entry; `find_all` on prose with
  chapters, ranges, ordinals ("1 John 4") and false friends ("Job 3 times", "Mark said").
- **Desktop:** `tests/ask_recents.rs` and `tests/empty_state.rs` move to the cache; a new test runs two
  `StudyStore`s against one local Convex backend and sees a note written in one appear in the other.
- **Sync load:** 1,000 clients subscribed to the feed while 100 write, against a dev deployment.
- **Start-up:** CI's startup probe stays within 5% of today's numbers with sync on. CI also records
  Convex's execution time for `highlights.set` so the Effect overhead is watched.

## Done when

- A note written on one Mac appears on another within a second, and survives editing on both while one
  is offline: the later edit wins, the earlier one is kept as a conflict copy, and the open editor says
  it changed.
- An existing user's notes, highlights, bookmarks and Recents migrate once, with nothing lost or
  duplicated, and a second device with its own different notes keeps both sets.
- `cargo xtask schema` is the only way a record shape changes, and CI proves it.
- Every Convex function is an Effect handler, and the proxy's tests still pass.
- Reading, notes and Recents work with the network off, and start-up time hasn't moved.

## Risks

- **The `convex` crate on tokio** adds weight to an app whose brand is speed. Keep it on its own thread
  after the first frame, and measure binary size and start-up in CI.
- **Effect's cost inside a V8 isolate** is unmeasured here; the spike measures it, and CI keeps
  measuring it.
- **The per-user sequence serializes a user's writes.** A person's own write rate is low, but workflows
  (transcription, later extraction) write too; keep their writes small and few.
- **One note per verse becomes many.** The popover and margin keep today's behaviour for the verse's own
  note; other items appear as library marks in M2.
