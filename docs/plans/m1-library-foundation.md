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
   script. Update `CLERK_URL` and `CLIENT_ID` (`crates/core/src/auth.rs`) and their defaults in
   `server/scripts/clerk-test-token.ts`, the proxy's `CLERK_*`
   variables and Convex's `CLERK_JWT_ISSUER`. Activate Clerk's Convex integration. Deploy the
   server to the production deployment `expert-rabbit-665` (`api::CONVEX_URL`, which release builds
   use) and set `CLERK_JWT_ISSUER` there before any build that signs in ships; today it has no
   functions.
   Launch a team-signed build once over a Keychain item written by the previous release (the
   session item changed shape in M1): macOS ties an item to the binary that wrote it, and an
   unsigned build reading another's item hangs on an invisible Keychain prompt, so check that a
   signed one starts signed in before shipping.
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
   As built (pass 2): `lib/allowance.ts` holds every plan check, count and reservation (Ask rounds,
   sermon seconds, storage) and the `/v1/me` summary; `effectServerMutation` / `Query` / `Action`
   (with `ServerCaller`) and `effectInternalMutation` wrap the proxy's and the crons' functions. On
   them so far: `usage.*`, `users.me`, every `sermons.*`, `account.purge` and `library/purge`, all
   reading time from `Clock`. Purging is a `Purge` service (`lib/purge.ts`: an account's next step,
   the tombstone sweep, the orphaned-blob sweep) whose steps are Effect functions, not one
   `Effect.promise` around an imperative body. A denial is still returned as the `Denial` value the
   proxy maps, not thrown. Still plain, and next: `entitlements.upsert`, `webhooks.*`,
   `users.setStripeCustomer` / `billingProfile`, `maintenance.*` and `admin.*`.
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
   - `library/highlights.set({ verse, color | null, baseSeq, createdAt? })`, `library/bookmarks.set({ verse, label | null, baseSeq, createdAt? })`;
     a stale `baseSeq` keeps the server's and answers `conflict`; clearing what is already clear
     answers `ok` with the record's current `seq` (0 for none) and consumes no sequence number;
   - `library/chats.save({ uid, title, fileUid, baseSeq })`, `library/chats.remove({ uid })`: the
     conversation JSON is uploaded and attached first as a `chat` file owned by the chat, so it counts
     toward storage;
   - `library/items.updateDetails({ uid, title?, details?, sermonUid?, offsetMs?, baseSeq })`.
     Built: a field given replaces the stored one and `null` clears it; a `sermonUid` must name a
     live sermon item other than this one; it answers a `WriteAnswer`, and never touches a note's
     text or anchors. In the cache, `StudyStore::set_item_details` edits a non-note item and the
     outbox sends it as `updateDetails` (the uid-keyed `note` outbox kind covers every item).
7. **The change feed:** `library/sync.changes({ after })` returns every synced record with `seq > after`,
   tombstones included, in `seq` order, up to 500 records or 2 MB, with the last `seq` and the server's
   time, or `resync` when `0 < after < userLibrary.purgedThrough` (a device at 0 has nothing to
   delete). Records are in the generated
   shape (items with their `anchors` and `files` arrays, file `url`s, `uid`s, no `_id`s).
8. **Files:** `library/files.uploadUrl()` and `library/files.attach({ uid, ownerUid, role, storageId,
   sha256, partIndex?, page? })`, which checks the storage limit through `Allowance`, refuses with
   `too_large`, and records the file's permanent `url`.
9. **`/v1/me`** gains `library: { storageUsed, storageLimit }` ([backend.md](backend.md#get-v1me)).
   Built: `plans` gains `storageBytes` (1 GB Free, 25 GB Pro) and `keepAudio`, optional in the schema
   until `admin:seed` (which now fills fields a plan row lacks) has run on every deployment; a file
   over the limit is refused `too_large` and isn't attached. Its upload stays in `_storage`,
   because deleting it inside the failing mutation is rolled back; nothing counts it, and a daily
   cron (08:15 UTC, `library/purge.orphans`) deletes blobs more than a day old that no `files` row
   or sermon transcript points at, looking back three days (indexes `files.by_storage`,
   `sermons.by_transcript`).
10. **Purge and tombstones.** Replace `account.purge` with the scheduled chain (`library/purge.step`, 500
    records a run) over every user table and file, ending with the `users` row
    ([backend.md](backend.md#deletion)). A daily cron hard-deletes tombstones older than 30 days with their
    files and index rows. `DELETE /v1/account` and the Clerk webhook start the chain.
    Built (`library/purge.ts`): the first step runs inside `account.purge`, so a small account is
    gone when the request returns, and each later step is scheduled; the tombstone cron
    (08:00 UTC) reads a `by_deleted` index on each synced table, 500 a run, rescheduling itself
    while more remain, and raises each user's `purgedThrough` to the highest `seq` it deleted.

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

   As built, migration 1 (pass 1) is the highlight and bookmark cache with `outbox`, `sync_state` and
   `conflicts`; migration 2 (pass 2) adds `items` (the record as JSON beside `kind`, `seq`, `deleted`),
   `item_anchors` (per chapter), `files` (a local `path` once fetched) and the chat columns (`uid`,
   `seq`, `deleted`, `file_uid`, `url`, `fetched`; the local order moves to `ord`), turns each note
   into the verse's own note item, drops `notes`, and queues notes and chats once. The outbox's `key`
   loses its type there, so it holds a verse id or a uid; a note's or chat's arguments are read from
   the cache when it's sent, so an edit only bumps its entry.

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
   Built for highlights and bookmarks (pass 1, October 20). The engine sends nothing until the
   feed has caught up once, so a device pulls before it uploads. The outbox goes strictly in
   order: a refused change holds the ones behind it, and the footer shows its error with Retry.
   The `convex` crate reconnects on its own and never errors a subscription for want of a network,
   so "offline" comes from its `WebSocketState` channel (`Backend::connected`), and the engine
   backs off as a whole while offline rather than per change. The cache keeps tombstones (a
   `deleted` flag and the `seq`) so a later edit is made against them. Highlights and bookmarks
   carry `createdAt` (server milliseconds): `set` takes the device's own creation time and uses it
   only for a new record or a tombstone set again, otherwise the server's clock, so a migrated
   library keeps its dates on every device; the cache stores the feed's `createdAt`, moved by the
   clock offset, instead of the time it arrived. The offset comes from `library/sync.now` (a mutation, so never a
   cached answer) on every connect, not from the feed's `serverTime`. A device's own
   record over one the server already has, made without a copy (`baseSeq` 0: migration, or an
   edit made before the feed first caught up), is answered `conflict` and the server's stands,
   which is the "skipped" of [Migrating](library.md#migrating-whats-on-the-device); when the two
   differ the conflict is recorded like any other, so an edit is never lost unannounced. The `convex` crate turns
   on serde_json's `preserve_order` for the whole build, so the Ask contract's hash and the record
   generator sort keys themselves (`contract::sorted`).
   Notes, chats and files follow (pass 2, October 20). Every answer to a note or chat write is a
   `WriteAnswer` (records.rs, generated into `shared/records.ts`): `ok` with the stored record, or
   `conflict` with the record that stood and, for a note edit, the conflict `copy`. Each record is
   exactly what the feed sends at its `seq`, and the writing device stores it (unless the note was
   edited again in flight), so its row, times included, equals every other device's; this device's
   note becomes the server's copy (an edit made while the first was in flight goes on as an edit of
   the copy). Saving a note's unchanged text is no write at all. A chat's conflict keeps the server's and isn't listed (a conflict names a verse).
   A chat is saved by uploading its conversation as a `chat` file named from the chat's uid and the
   bytes' SHA-256, so a retry attaches the same file and an unchanged one isn't uploaded again. Chats
   from another device are listed at once and their conversations fetched eagerly, like text files,
   rather than on demand: they're small and Recents should open offline. `Synced` waits for those
   fetches. A second device's migrated note is dropped instead of sent when the library already has
   the same text on the same anchors. The desktop keeps a chat's local row id as its handle (only the
   engine uses `uid`), so Recents didn't change. `Item` has no photo `pages` until M4.
   `Item` and `Chat` carry `updatedAt`, server-stamped on every write, so the cache's `updated` (and
   the export) is the real edit time on every device, not the time the record arrived. A conflict
   copy whose original is gone (deleted, or not in the cache) is an ordinary note on its verse and
   can be the verse's own. The `items.seq` column is the server's and the one source: the record
   JSON's `seq` is replaced by it on load. Every cache transaction begins `IMMEDIATE`, because a
   read-then-write transaction fails \"database is locked\" rather than waiting when the other
   connection committed in between. A Convex call fails as offline once the socket has been down
   3 s, instead of waiting out the crate's reconnects for the 20 s call timeout, so the status is
   Offline at once whatever is first in the outbox.
4. **`reference::find_all(text) -> Vec<(Range<usize>, Reference)>`**, moved from
   `apps/desktop/src/pages/notes.rs` `mentioned()` and extended to whole chapters and ranges, with the
   existing tests.
   Built (October 20): a verse or range is found after up to three words naming a book (typos
   allowed only in one word of four letters or more); a whole chapter needs a capitalized full name
   or alias, not an everyday word like "Is", followed by punctuation, the end, or "and"/"or", so
   "Job 3 times" and "Mark said 12" aren't references. A note write stores every reference found as
   a `typed` anchor beside its `user` anchor (a chapter anchors all its verses); `mentioned()` in the
   desktop now calls it.
5. **`api.rs`** gains `CONVEX_URL` (overridable with `LINGER_CONVEX_URL`) beside `API_URL`, and
   `ApiError` becomes the generated enum.

## The desktop app

Pass 2's first half changes only the cache under the existing UI: the popover and notes page edit the
verse's own note through it, and Recents sync. What follows for the notes page and popover (a conflict
copy shown beside its note, "edited on another device"; typed anchors from `find_all` are built) and
`items.updateDetails` come after pass 2's second push (purge, Allowance, `/v1/me`, export), with the
library consent sheet.

1. **Start sync after sign-in** from `Account`. **Sign-out** waits for the outbox (or asks), then deletes
   the cache and cached files, keeping recordings that exist only on this Mac unless the user deletes them
   ([library.md](library.md#the-sync-engine)). The desktop already requires
   sign-in before reading (`sign_in_required`).
2. **Sync status** in the sidebar footer: a quiet "Synced", "3 changes waiting" when offline, and the error
   with Retry when something is refused.
3. **Notes** (`note_popover.rs`, `pages/notes.rs`): the popover edits the verse's own note item; the notes
   page lists note items, and a note carries every anchor `find_all` finds. "Edited on another device"
   when the feed replaces the open note, and a conflict copy shown beside the note it collided with.
   Built: core keeps each verse's conflict copies (`StudyStore::conflict_copies`) and marks a note
   whose text the feed changed this session (`Note::edited_elsewhere`); the page's meta line then
   reads "Edited on another device …", and the open editor takes the new text unless there's unsaved
   typing. A copy is a card above the verse with "Use This Version" (its text becomes the note and
   the copy is deleted) and "Delete Copy". Linked chips show each reference as written ("1 John
   4:7–8", "Romans 5"), jumping to its first verse.
4. **Recents** (`sidebar.rs`, `ask_panel.rs`): `uid`s instead of row ids; a conversation is saved as a
   file through the outbox and downloaded when opened on another device.
5. **Migration** ([library.md](library.md#migrating-whats-on-the-device)): a one-time sheet ("Bringing
   your notes into your library") with progress, then the library consent sheet
   ([backend.md](backend.md#privacy-and-consent)). Recordings follow in M1b, through the same sheet.
   The Ask consent text drops "conversations stay on the device".
   Built: the library consent sheet shows once, signed in, after onboarding; sync starts only after
   "Turn On Sync". "Keep on This Mac" leaves the library here, unsynced (nothing is uploaded), and
   the sidebar footer then reads "Only on this Mac · Turn On Sync", which shows the sheet again. The
   answer is a per-device setting (`library_consent`). Departure: the migration sheet ("Bringing your
   notes into your library") isn't separate; the outbox's existing migration runs once sync starts,
   with progress in the footer.
6. **The CLI** (`cli.rs`) reads notes from the cache.
7. **Export** (`linger export --json`, the user's backup) lists every live note item, several to a
   verse allowed, each `{ reference, book, chapter, verse, text?, note, created, updated, uid,
   conflict_of? }`. Import updates the item a `uid` names when this cache has it, skips a note already
   on its verse word for word, lets an export from before uids overwrite the verse's own note with
   its first note there, and otherwise adds another note item, so a conflict copy comes back as a
   copy.

## Owed

Before M1 ships: a generated answer type for the verse-keyed writes (`highlights.set`,
`bookmarks.set`), so the engine reads no answer JSON by hand.

Done since (October 20): the consent sheet, the notes UI and `items.updateDetails` (above); the
handlers' `satisfies … as` casts became annotated return types; `lib/purge.ts` no longer claims
tests set `Clock` (they fake `Date`, because the sweeps compare Convex's `_creationTime`); and
`local_time` takes the server's second and moves it by the offset rounded to whole seconds, instead
of rounding the moved time, so offset jitter between devices (tens of ms) no longer puts one stamp
on different seconds (Linux prover: stamp 1792505334916, A 14:08:54Z, B 14:08:55Z). The cache still
stores seconds; that is the departure from "export from the server's milliseconds": it fixes every
record type, highlights included, with one rule, and two devices can still differ only if their
measured offsets fall either side of ±0.5 s.

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
