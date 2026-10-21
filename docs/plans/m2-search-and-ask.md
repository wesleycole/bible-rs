# M2: Search and Ask over the library

Status: not started; M3a (iOS on M1's surface) runs first. Depends on: M1, M1b. Design: [library.md](library.md#search-and-ask),
[backend.md](backend.md#the-ask-contract).

**Goal.** Ask answers from the user's library as well as the Bible, cites each library claim, and the
reader shows what the library holds on every chapter. After M2, "what did I hear about this chapter?"
works on the desktop, and the evaluation proves it.

**Not in M2:** photos (M4; the tools handle them, but there are none yet), outside sources (M7),
vectors (only if the eval below asks for them).

## Convex

1. **Chunks.** Add the server-only `chunks` table with its search index
   ([library.md](library.md#the-data-model)).
   `library/chunk.ts`, a pure module with no `ctx`, rebuilds an item's chunks whenever its text or
   text file changes:
   - notes: one chunk;
   - sermons: transcript segments joined into paragraphs of about 200 words on speaker and pause
     boundaries, keeping `startMs` and `endMs`.
2. **Anchors from text.** `notes.upsert` already takes the client's anchors (from
   `reference::find_all`); the server validates them. Transcripts get anchors from the digest.
3. **The digest step** ([library.md](library.md#the-digest-step)): a workflow step after transcription
   and on demand for existing sermons, through the Gateway with `zeroDataRetention` and the `digest`
   model row, with a JSON schema for `{ references: [{ ref, atMs?, page? }], outline, summary }`. Each
   reference is validated against the grid and becomes an anchor at its time or page. A ledger row with
   `kind: digest`. Set `AI_GATEWAY_API_KEY` on each deployment.
4. **Backfill.** A one-off internal action chunks and digests every existing sermon and note.
5. **`library/search.search`** (an action, so neither reactive nor cached): anchors, then the text
   index, merged with reciprocal-rank fusion (a pure module), focus and anchored chunks boosted,
   grouped by item ([library.md](library.md#librarysearch)).
   `library/items.read({ uid, fromMs?, minutes?, page? })` (query) for `read_item`, capped at about
   2,500 words like `read_sermon` today.
6. **The chapter query:** `library/items.forChapter({ book, chapter })` returns the items anchored to
   each verse, for the margin and the "In your library" list. The native apps answer it from the cache's
   anchor index; the query exists for the web (M8).
7. **Export:** `library/export.start()` builds a zip (Markdown per item with references and sources,
   plus the original files) in an action, stores it as an `export` file, and returns its link; deleted
   after a day. The purge chain covers exports and the new `chunks` table.

**Built (October 21), where it is smaller than the above:** `items.read` reads an item's text from
its `chunks` rather than the text file, so it stays a query (a query can't read storage) and a sermon's
window is its paragraphs by time, each stamped. `forChapter` answers each anchor's whole range, mapped
back from the chapter's index row, so a range that starts in the chapter before says so. The digest
calls the Gateway's `/v1/messages` with `AI_GATEWAY_API_KEY` (`AI_GATEWAY_URL` overrides the host) and
forces one tool whose input schema is the digest's shape; the `digest` row in `models` is seeded by
`admin:seed`, so dev needs a seed before the step runs. The digest's cost is a `ledger` row of kind
`digest` and nothing more: it doesn't count against the person's Ask allowance in `usage`. The
digest runs for trial transcripts too. The backfill is one internal action over every live note and
sermon, sequential, a failed digest logged and skipped; it is run once from the dashboard, not
scheduled. The export zip is stored-only (no compression: audio and photos don't compress and text is
small), the Markdown page gets the transcript laid out a stamped line a turn, and each export schedules
its own deletion a day on rather than relying on a cron. `search.search` searches reviewed and
unreviewed chunks alike and marks each hit `unreviewed`; the text index is Convex's own, with no
prefix or typo tolerance beyond what it gives. Known limit: `export.start` reads every file of the
library into memory and writes the zip as one buffer without ZIP64, so a library of many hour-long
sermons could pass an action's memory; it will need streaming or a split before that. `digest.items`
(the backfill) collects items across every user, which is fine run once from the dashboard and must
not be scheduled as written.

## linger-core

1. **Contract `ask/5`** in `contract.rs` and `chat.rs`:
   - `search_library { query, passage?, kinds?, source?, limit? }` and
     `read_item { item, from?, minutes?, page? }`
     replace `search_sermons` and `read_sermon`. `chat::Tools` loses `sermons`; the tools send their
     calls to the sync thread's `LibraryBackend` through `SyncHandle::call` and block on the reply
     ([library.md](library.md#the-sync-engine)), so there is no second Convex client.
   - The three prompt rules and the `{{item <uid prefix> [mm:ss | p<n>]}}` citation
     ([library.md](library.md#the-prompt-and-citations)). `chat::segments` parses `{{item …}}` and keeps
     parsing `{{sermon …}}` for old chats.
   - `tools_note` says how many library items the user has and how many touch the passage being read.
   - `cargo xtask contract` writes `ask-v5.json`; the proxy serves `ask/5` and `ask/4` and drops
     `ask/3` ([backend.md](backend.md#the-ask-contract)).
2. **Offline:** with no network, `search_library` answers from `search.rs` over the cached items' text
   and text files, ranked as Bible search is, and says it's offline.
3. **Audio playback** for sermon items: `symphonia` (AAC, MP4 and FLAC) through `rodio`, playing a file cached
   in `<data>/files/` (downloaded on first play; about 22 MB an hour) with seeking by time.

**Built (October 21), where it is smaller than the above:** the tools reach the library through
`chat::UserLibrary`, a trait `SyncHandle::user_library()` implements over the sync thread (an
`Action` for `search.search`, a `Query` for `items.read`) and its own cache connection; the fake
server implements the same two functions over the same pure code, so one test runs a sermon's search
and read online and offline. `read_item` and `search_library` take an item's id prefix as a citation
gives it and the cache widens it to the whole id when exactly one live item matches. Offline search is
Bible search's word matching (`search::Query`, every word present, `-word` excluded) over each cached
passage (a note's paragraph, a transcript's turn), items anchored to the passage asked about first and
then newest, three hits an item and the server's caps; a sermon's excerpt is about sixty words with its
moment interpolated inside the turn. There is no offline search over photographed items' OCR until the
desktop can capture them. Offline `read_item` answers from the cache too. `tools_note` counts items
and the chapter's anchored items from the cache; signed out, the two tools answer with an error that
says so instead of being hidden, so the contract stays one byte-identical prompt. The desktop panel
labels an `{{item …}}` citation of a sermon recorded on this machine from its transcript and gives it
the recording chip; any other item reads "an item in your library" until the library page lands
(desktop item 1). Audio playback (item 3) is not built: nothing plays audio until the sermon page of
desktop item 1, and `rodio` needs ALSA headers that CI and the orb's setup do not install, so it
goes in with that page behind a `playback` feature. Contract tests: the proxy's `ask/5` and `ask/4`
list, the tool names, and the drift test over `ask-v5.json`.

## The desktop app

1. **Citation chips** open the item: a sermon opens its page at that second and starts playing; a note
   opens in the notes page. Chips for deleted items say "no longer in your library".
2. **A library page** (`pages/library.rs`): items by kind and date, a search box over the cache, and an
   item view. A sermon's view shows the summary, outline, transcript with times (click to play from
   there) and its anchors.
3. **The reader's library marks** (`reader/mod.rs`): a small glyph in the run of margin marks
   (`634-655`) for verses with library items other than the verse's own note, opening a popover that
   lists them; and an "In your library" line under the chapter heading ("4 items") that opens the list.
4. **The Ask panel** shows "7 items on Romans 8" before the first question, and **Save to library** on
   an answer, which makes it a note anchored to the passages the answer cites.
5. **Export** in Settings → Account: "Export your library" downloads the zip.

## Evaluation: `cargo xtask eval-ask`

- **The library fixture** (`xtask/eval/library/`): about 20 sermon transcripts, 30 notes and (from M4)
  15 page photos, all written for the test, quoting only public-domain translations, so no copyrighted
  text enters the repository. A loader seeds it into a dev deployment as a test user.
- **The questions** (`xtask/eval/questions.jsonl`, new): about 50 Bible questions answerable from
  public-domain text, and 40 library questions, each with the item `uid`s a good answer cites.
- **Scores:** citation validity (every `{{Book C:V}}` was read with a tool, and every `{{item …}}` was
  returned by a tool), **library citation recall** (expected items cited), separation (the answer
  doesn't present a note's claim as the text's; checked by a grader prompt and spot-read), tool rounds
  and length. Results go to `xtask/eval/results/<date>-<model>.json` for comparison.

## Tests

- convex-test: chunking boundaries and times; digest output with a bad reference drops it; search
  never returns another user's chunks; anchored chunks rank above text-only matches for a passage
  query; `forChapter` returns ranges that start in the previous chapter.
- Core: `{{item …}}` and legacy `{{sermon …}}` parsing; the tools against `FakeBackend`; offline
  search over a cached fixture; the contract drift test.
- Desktop: a chip for a sermon opens the item at the right second; the margin shows a mark on a verse
  covered by a range anchor that starts in the previous chapter.

## Done when

- The eval cites the expected library item in at least 85% of library questions, with citation
  validity at 100%. If text search misses 85%, add vectors ([library.md](library.md#vectors-if-the-eval-asks-for-them))
  and run it again.
- A question about the open chapter finds a sermon that cites it only in its digest.
- The margin and "In your library" line appear for every chapter with library items, and opening a
  chip plays the sermon from the cited second.
- Export produces a zip that a person can read without Linger.

## Risks

- **Digest quality** decides recall for sermons. The eval measures it; the `digest` row can move to
  Sonnet if Haiku misses references.
- **Text search's limits** (no typo tolerance, last-word prefix only) may cost recall on spoken-word
  transcripts. The prompt asks the model to try two phrasings before concluding the library has
  nothing.
