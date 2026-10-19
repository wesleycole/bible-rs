# Linger context plan

Status: draft, October 19, 2026. This plan extends [backend-plan.md](backend-plan.md). It replaces that
plan's local-only storage, its Vercel Blob sermon store and its "sync later" item. Ask's proxy,
accounts, payments and licensing stay as they are there.

## What Linger is now

Linger is a **personal Bible context manager**. Reading the Bible is the main screen. Around it,
the user builds a library from what they study:

1. Sermons recorded on iOS or macOS.
2. Photos of handwritten notes.
3. Photos of sermon slides, taken while a sermon is being recorded or not.
4. Photos of book and commentary pages, circled and underlined with a finger to mark what matters.
5. Notes written in the app while reading.
6. Trusted outside sources: the lexicons, cross-references and people and places Linger already
   bundles, plus public-domain commentaries and confessions.

When the user asks a question while reading, Ask answers from that library and the Bible, and
says which item each claim came from. Everything is available on every device: photograph a page
with the iPhone while reading on the Mac, and Ask on the Mac can use it seconds later.

The library is **personal**. Nothing is shared between users. The schema leaves room for a church
library later (see [Later](#later)).

## Summary

| Decision | Choice | Why |
|---|---|---|
| Where the library lives | **Convex**, for every personal record and every file (audio, photos, transcripts). No Vercel Blob | One store, one auth check, live queries for handoff; Convex file storage covers the files |
| How devices sync | Apps talk to **Convex directly** with Clerk tokens. Each app keeps a local cache and an outbox for offline captures | Live queries make a photo taken on the iPhone appear on the Mac without a sync protocol of our own |
| The unit | A **library item**: the original (audio, photo or none), its text, its **verse anchors** and where it came from | One shape for sermons, notes, slides and pages; verse anchors make "everything on Romans 8" an exact lookup |
| Retrieval | **Verse anchors and full-text search** in Convex, with vectors added only if the eval needs them; two Ask tools, `search_library` and `read_item` | Anchors find what is about this passage; text finds what is about this idea, and the model rewrites its own queries |
| Photos to text | **Claude vision** through the AI Gateway, then a review screen | Reads handwriting and marked-up pages far better than OCR; the user corrects before it counts |
| Marks on photos | A **stroke layer** kept apart from the photo | Searchable "focus" text, re-extraction later, and the clean page stays available |
| Vendor calls for the library | **Convex actions** call AssemblyAI and the Gateway | The files are in Convex already; the Vercel proxy keeps only Ask |
| Ask | Unchanged: the Vercel proxy with OIDC, metering and the tool loop on the client | Proven under load; the new tools are one more client-side call |
| Web | **Reader and Ask only**, signed in. No recording or capture | The most limited client, as decided |
| Sharing | None for now. Every read goes through one access helper | Adding a church library later changes one function |

```diagram
┌──────────────────────────┐  photo / audio / note  ┌──────────────────────────────────────────┐
│ iPhone (SwiftUI)         │ ─────────────────────▶ │ Convex                                   │
│ camera, scanner, pencil, │ ◀──── live queries ─── │  library: items, chunks, anchors, assets │
│ recording, reader, Ask   │                        │  files: audio, photos, transcripts       │
└──────────────────────────┘                        │  workflows ─▶ AssemblyAI (sermons)       │
┌──────────────────────────┐                        │           ─▶ AI Gateway (vision, digest) │
│ Mac (egui)               │ ─── Clerk JWT ───────▶ │                                          │
│ reader, recording,       │ ◀──── live queries ─── │  search: verse anchors + full text       │
│ Continuity Camera, Ask   │                        └──────────────────────────────────────────┘
└──────────────────────────┘                                          ▲ library.search
┌──────────────────────────┐                                          │ (client-run Ask tool)
│ Web (reader + Ask)       │ ── Ask rounds ─▶ Vercel proxy ─▶ AI Gateway ─▶ Claude
└──────────────────────────┘
```

## What changes from the backend plan

- **Local-first ends for personal data.** The Bible, its search, the lexicons and cross-references
  stay on the device and work offline, as now. Notes, highlights, bookmarks, Recents, recordings
  and photos move to Convex, which holds the master copy. Devices cache it.
- **Convex file storage replaces Vercel Blob.** `server/src/services/Blob.ts`, the
  `/v1/dev/blob/*` route and `sermons.blobPath` go. Sermon audio becomes an `_storage` id.
- **Sermon audio is kept**, so a recording plays on any device and a transcript line can jump to its
  moment. Until now the audio was deleted once the transcript was done.
- **Transcription moves from the proxy to Convex.** The `/v1/sermons` endpoints become Convex
  functions. AssemblyAI calls a Convex HTTP action back. The allowance logic (`reservations`,
  `usage`) is already in Convex and stays.
- **The proxy keeps Ask only**, plus billing and its webhooks.
- **Convex holds two vendor keys**, `ASSEMBLYAI_API_KEY` and an `AI_GATEWAY_API_KEY` for extraction
  and digests, set with `npx convex env set`. The no-key OIDC path stays with the proxy, because only
  Vercel deployments get the OIDC token. Those calls are short and don't stream, so a key is simpler
  than routing each one through the proxy.
- **The privacy story changes.** "Conversations stay on the device" and "audio is deleted after
  transcription" are no longer true. See [Privacy](#privacy-deletion-and-export).
- **The iOS sign-in risk goes away.** App Review guideline 5.1.1(v) worried the backend plan,
  because reading isn't account-based. A synced personal library is a substantial account-based
  feature, so requiring sign-in is justified.

## The library model

Every record has a `userId` and goes through one `canRead(ctx, doc)` / `canWrite` helper. Nothing
reads by id without it.

**Verse ids.** A verse is the integer `book × 1,000,000 + chapter × 1,000 + verse` on the canonical
66-book grid that the reader, highlights and notes already use (`BookId` 0–65). John 3:16 is
`42_003_016`. Ids sort in canon order, so a range is two numbers.

| Table | Fields | Indexes |
|---|---|---|
| `items` | `userId`, `kind` (`sermon`, `handwritten`, `slide`, `page`, `note`), `title`, `status` (`uploading`, `processing`, `review`, `ready`, `error`), `text` (the user-approved text), `summary?`, `sourceId?`, `sermonId?` + `offsetMs?` (a slide taken during a recording), `capturedAt`, `updatedAt`, `deletedAt?` | `by_user_updated`, `by_user_kind`, `by_sermon` |
| `sources` | `userId`, `kind` (`book`, `preacher`, `church`, `website`), `title`, `author?`, `publisher?`, `year?` | `by_user_kind` |
| `chunks` | `userId`, `itemId`, `ordinal`, `text`, `startMs?` / `endMs?` (sermons), `focus` (the user circled it), `embedding?` | search index on `text` (filter `userId`), `by_item`; a vector index only if phase B adds one |
| `anchors` | `userId`, `itemId`, `chunkId?`, `book`, `chapter`, `from`, `to` (verse ids), `origin` (`user`, `parsed`, `model`) | `by_user_chapter` [`userId`, `book`, `chapter`] |
| `assets` | `userId`, `itemId`, `role` (`audio`, `photo`, `thumbnail`, `transcript`), `storageId`, `contentType`, `bytes`, `width?`, `height?`, `durationMs?`, `sha256` | `by_item`, `by_sha` |
| `marks` | `userId`, `itemId`, `assetId`, `crop?`, `strokes` (normalized 0–1 points, pen kind, colour) | `by_item` |
| `highlights`, `bookmarks` | as in `study.sqlite`, plus `userId`, `updatedAt`, `deletedAt?` | `by_user_chapter` |
| `chats` | Recents: `userId`, `title`, `messages` (JSON), `updatedAt`, `deletedAt?` | `by_user_updated` |
| `positions` | `userId`, `device`, `reference`, `translation`, `at` (the last place read, for handoff) | `by_user` |

Why it's shaped this way:

- **Anchors are stored once per chapter they touch.** A range across a chapter break becomes two
  rows. The reader needs "what is anchored to this chapter" for its margin and "In your library"
  list, and one indexed query per chapter answers it. Ask filters the same rows by `from`/`to`.
- **Chunks are the unit of search and citation.** A sermon is chunked by transcript paragraphs
  (about 200 words, with timestamps). A page is chunked by paragraph, and the circled passages
  become their own chunks with `focus: true`, ranked first. A note is one chunk.
- **Verse notes become items** of kind `note`, anchored to their verse or range. The reader's note
  popover edits the item. The `notes` table in `study.sqlite` becomes the cache.
- **Sources are reused.** The first page photographed from a book asks for its title and author.
  Later pages pick the book from a list, so "what does Stott say about this?" can filter by source.
- **Tombstones.** Deletes set `deletedAt` so other devices drop the record from their caches. A cron
  hard-deletes after 30 days, along with any files.
- **Documents stay small.** A one-hour transcript is about 60 KB of text, well inside Convex's
  document limit, but its segments live in a `transcript` file and its chunks are rows. Items
  never hold large blobs.

### Ask conversations are not context

Recents sync, but Ask's answers are not searched as library material. Feeding the model's own
answers back as grounding would compound its mistakes. A "Save to library" action on an answer
makes it a `note` the user chose to keep.

## Sync and handoff

- **Auth.** `convex/auth.config.ts` trusts the Clerk "Linger" application for both token kinds the
  proxy already accepts: Clerk session tokens from iOS and the web, and the desktop's OAuth access
  tokens. Library functions take the user from `ctx.auth.getUserIdentity()`, never from an argument.
  The proxy's `serverMutation` secret stays for proxy-only functions.
- **Clients.**
  - **Desktop and iOS share one sync engine** in a new `crates/core/src/sync.rs`, on the `convex`
    Rust crate (0.10): subscriptions for the current chapter's anchors, the library list and Recents,
    plus the cache and outbox below. iOS reaches it through `crates/ffi`, with a callback that hands
    SwiftUI each change. The token comes from `set_auth_callback`, which fetches a fresh one on every
    reconnect: the desktop's OAuth access token, or Clerk's iOS session token.
  - The official Swift client (ConvexMobile 0.8, and Clerk's `clerk-convex-swift` 0.1) was the
    alternative. It is built on the same Rust crate, but both are pre-1.0, and using it would mean
    writing the cache, outbox and conflict rules twice.
  - Web: the Convex React client with `ConvexProviderWithClerk`.
- **Cache and outbox.** Convex has no offline sync (its FAQ says so; Replicache was the suggested
  workaround), so each native app keeps:
  - **A cache** in `study.sqlite`: items, chunks' text, anchors, highlights, bookmarks and Recents.
    Reading, the margin and the library list work offline from it.
  - **An outbox** for captures and edits made offline. A photo is saved locally with its marks and
    uploaded when the network returns. Each outbox entry carries a client-made id, so a retry never
    creates a second item.
  - **Conflicts are last-writer-wins per record** by `updatedAt`. One person edits their own notes,
    so this rarely matters. An edit to an item deleted elsewhere is dropped.
- **Handoff.**
  - **Captures appear everywhere live.** A photo taken on the iPhone appears in the Mac's library as
    "Reading your photo…", then as ready. Ask on the Mac can find it as soon as it is ready.
  - **Reading position.** `positions` lets any device offer "Continue Romans 8 from your iPhone".
    On Apple platforms an `NSUserActivity` also puts Linger in the Dock and App Switcher for
    Handoff.
  - **Continuity Camera on the Mac.** "Take Photo" and "Scan Documents" from the iPhone straight
    into the Mac app, through `NSMenuItem.importFromDeviceIdentifier` and an AppKit responder
    bridged with objc2. The photo then follows the same review flow as one taken on the iPhone.
    Drag-and-drop and a file picker work on the Mac too.

## Capture

### Sermons

- **macOS** records today: 30-second 16 kHz WAV chunks from the microphone or system audio, with the
  preacher and church in `recording::Details`.
- **iOS** records with `AVAudioEngine`, with background audio, a Live Activity showing elapsed time,
  and interruption handling (calls, Siri).
- **Encoding.** Kept audio is encoded to **AAC, mono, 48 kbps** (about 22 MB an hour) before upload,
  on both Apple platforms (`AVAudioConverter`), instead of 115 MB an hour of WAV. AssemblyAI accepts
  it. The Mac keeps WAV chunks while recording, for crash safety, and encodes at the end.
- **Upload in parts, while recording.** A Convex upload URL has no size limit, but each upload POST
  must finish within 2 minutes. A two-hour service is about 44 MB, which needs 3 Mbit/s of upload, more
  than church Wi-Fi or a phone signal reliably gives. So audio is encoded as **ADTS AAC** in
  5-minute parts (about 1.8 MB each) and each part is uploaded as soon as it closes, through the
  outbox. ADTS streams join by concatenation. Uploading while recording also means most of the
  sermon is already safe if the phone dies.
- **Play back M4A, not ADTS.** When recording ends, a Node action joins the parts and remuxes them
  (without re-encoding) into one M4A with its index at the front, using a pure-JavaScript MP4
  muxer. That file goes to AssemblyAI and to playback, and the parts are deleted. ADTS has no index,
  so players estimate its timeline from the bitrate. In a test, AVPlayer gave a 259-second ADTS file
  a duration of 238 seconds, about 8% short. A transcript timestamp an hour into a sermon would then
  land minutes from the sentence it belongs to. The same audio as M4A reported its exact duration.
- **Seeking streams from Convex.** File links answer `Range` requests with `206` and the exact bytes
  (tested on Convex Cloud), so AVPlayer and `<audio>` seek without downloading the whole sermon.
  Suffix ranges (`bytes=-N`) returned a `500` in the same test. Players use explicit ranges for
  M4A, but the playback tests should cover it.
- **Transcription.** A Convex workflow:
  1. Reserves the duration against the plan, as `beginSermon` does now.
  2. Gives AssemblyAI the file's `ctx.storage.getUrl` URL, with the same settings as
     `transcribe.rs`: Universal-3.5 Pro, speaker labels, and the 66 book names as key terms.
  3. On the webhook (a Convex HTTP action with a per-job secret), stores the transcript file,
     settles the real duration and deletes AssemblyAI's copy.
  4. Runs the **digest step** (below), then chunks and anchors the transcript.
- **Slides during a sermon.** A photo taken on the iPhone while it is recording is attached to that
  sermon automatically, with `offsetMs` set to the moment it was taken. A photo taken on the iPhone
  while the Mac records is matched by time: if a recording is in progress on any device, the
  capture screen offers to attach it. `read_item` on a sermon returns its slides inline at their
  timestamps, so Ask sees what was on screen while each point was made.
- **Free users** transcribe on the device (SpeechTranscriber), as the backend plan says. The
  transcript is uploaded as text and goes through the same digest, chunking and anchoring.

### Photos

1. **Capture.**
   - iOS: the camera for slides and notes, and the document scanner (`VNDocumentCameraViewController`)
     for book pages, which straightens and crops them.
   - The user picks a kind: handwritten notes, slide, or book page. The extraction step checks the
     kind and can suggest another.
   - Mac: Continuity Camera, drag-and-drop or a file.
2. **Mark (optional).** A PencilKit canvas over the photo. Circle, underline or highlight with a
   finger or Pencil. Strokes are saved normalized to the photo's size, apart from the image.
3. **Upload.** The photo as JPEG at 2,048 px on the long edge (about 400 KB), its thumbnail, and its
   marks. The item is created as `processing`.
4. **Extract.** A Convex action sends Claude the photo with the marks drawn on a copy in a colour the
   page doesn't use, and asks for structured output:
   - `text`: the whole page or slide, in reading order;
   - `focus`: each marked passage, with how it was marked;
   - `references`: every Bible reference, normalized, including ones like "v. 12" resolved from the
     page's context;
   - `title`, and for pages the running head and page number when printed.

   Handwriting goes to Sonnet 5.5. Slides and printed pages go to Haiku 5.5, if the eval in phase C
   shows it reads them as well.
5. **Review.** The item becomes `review`. The user sees the photo beside the extracted text with the
   focus passages highlighted. They correct the text, confirm or edit the references, and pick or add
   the book. Saving makes it `ready`, then it is chunked and anchored. A setting can skip
   review for slides.

The original photo is always kept, so an item can be re-extracted when models improve.

### Notes in the app

The reader's note popover writes a `note` item anchored to the selected verse or range. A note can
also be written without a verse, from the library. References typed in a note ("see Rom 5:8") are
anchored too, through a new `reference::find_all(text)` that finds references in prose with the
parser the ⌘K palette uses.

### References: parsed, then checked

References come from three places, recorded in `anchors.origin`:
- the user (the verse a note is attached to, or the references confirmed on review);
- `reference::find_all` over text typed in the apps;
- the model, from photos and the digest step.

Every reference is checked against the canonical grid before it is stored: a book, chapter and verse
that exist, from ≤ to. `cargo xtask contract` exports the grid (verses per chapter) as JSON for the
Convex functions. A reference that fails is dropped and logged, never guessed.

### The digest step

After a sermon is transcribed, or a long page set is saved, Haiku reads it once and returns:
- the references the preacher read or cited, with the transcript times;
- a short outline;
- a one-paragraph summary.

The summary becomes `items.summary`, and each reference becomes an anchor on the chunk at that
time. This is what makes "what did I hear about Romans 8?" work for a sermon that never says
"Romans" in the paragraph that explains it.

## Retrieval and Ask

### Search

`library.search` is a Convex action. It takes `{ query, passage?, kinds?, sourceId?, limit }`:
1. **Anchors.** If there is a passage, find the chunks anchored to any verse in it, through
   `by_user_chapter`.
2. **Text.** The full-text index over chunks, filtered to the user.
3. **Vectors, if added.** The query is embedded and matched against chunk embeddings, filtered to
   the user.
4. **Merge** with reciprocal-rank fusion. Focus chunks and anchored chunks get a boost. Results are
   grouped by item, so one sermon doesn't fill every slot.

It returns chunks with their item's kind, title, source, date and timestamps. It works the same from
every client.

Step 3 exists only if phase B adds vectors (below). The embedding model would then be named in the
`models` table, and changing it would re-embed through a background workflow.

### New Ask tools (contract `ask/5`)

- **`search_library`** calls `library.search`. Its description tells the model that the library is
  what this user has heard, read and written: not Scripture and not necessarily right.
- **`read_item`** returns an item's text, or a window of it: a sermon around a time, with its slides
  inline, or a page with its focus passages marked.
- They replace `search_sermons` and `read_sermon`, because sermons are now items.
- The other tools (`get_passage`, `search_bible`, `original_words`, `lexicon`, `word_occurrences`,
  `cross_references`, `people_and_places`) are unchanged and still run on the device.

### Vectors, if the eval asks for them

Phase B starts with **anchors and full-text search** and measures citation recall on the eval.
Convex's text search has no typo tolerance and matches only the last word as a prefix, but the Ask
model writes the queries and can try several phrasings in one question. Anchors cover the commonest
case, a question about the open passage.

Vectors would add matches by meaning ("grief" finding a sermon about lament). They cost more than
the embeddings, though. Convex bills each vector search for the **whole index**, whatever the
filter, and every user's chunks share one index. At 1,000 Pro users with 1,500 chunks each,
512-dimension vectors make a 3 GB index, so each search costs 3 query-GB. That is about 16,000
searches a month inside the Professional plan's 50,000. Add vectors only if the eval shows a
meaningful gain, at 256 or 512 dimensions. Revisit if the cost grows with users, for example by
searching in a separate vector store or ranking a user's own embeddings in an action.

### Where the tools run

The tools run on the client, like the others. The client calls Convex with the user's token, so the
proxy never sees library contents except as tool results inside the conversation it already
forwards.

### The prompt and citations

The system prompt gains three rules:
1. **Use the user's library when it bears on the question**, and say so by name: "In Pastor Lee's
   sermon on March 3…", "Your notes on this chapter say…".
2. **Keep the user's material apart from the text.** When a note, sermon or book says something the
   passage doesn't support, say so gently and show the passage.
3. **Cite each library claim** with the chunk's citation marker from the tool result. The Ask panel
   renders markers as chips that open the item at that place: the photo with its marks, or the
   sermon at that second.

The request names the passage the user is reading, as now. The Ask panel shows how much of the
library touches it ("7 items on Romans 8") before the user types, from the same anchor query the
margin uses.

### Licensing

- **Bible text sent to the model stays public domain**, by the gate in the backend plan.
- **Library material is the user's own** for their private study. Their photos of books may contain
  copyrighted text, including quoted modern translations. API.Bible's AI restriction covers text
  obtained from API.Bible, not a user's photo of a book they own. Personal, private use is the
  defensible case, so:
  - nothing in one user's library is ever visible to another, or to any shared index;
  - marking encourages excerpts ("circle what matters"), not scanning whole books;
  - **every model call is zero-retention and never used for training:** Claude through the
    Gateway with ZDR for Ask, extraction and digests, and AssemblyAI with its job deleted when the
    transcript is stored (as now). Confirm AssemblyAI's no-training terms in writing.
- ZDR settles what the vendors keep. It doesn't settle copyright. Before launch, ask a lawyer to
  confirm this reading for book photos and sermon audio.

## External sources

Bundled today: STEPBible's tagged Hebrew and Greek with Abbott-Smith, BDB and Strong's;
OpenBible.info cross-references; people and places.

Next, public-domain works, each recorded in `LICENSES/README.md` as now:
- **Commentaries:** Matthew Henry, Jamieson-Fausset-Brown, Barnes' Notes, Keil & Delitzsch, Calvin's
  commentaries (19th-century translations).
- **Reference:** Easton's Bible Dictionary, Nave's Topical Bible.
- **Confessions and catechisms:** Apostles', Nicene and Athanasian Creeds; Westminster Confession and
  Catechisms; Heidelberg Catechism; Belgic Confession; Canons of Dort; 1689 London Baptist Confession.
  Several traditions, so Linger doesn't pick one.

**Where they live.** The commentaries are tens of megabytes even compressed, too much to bundle with
every install and too much for the web to download. They go in **shared, read-only Convex tables**
keyed by verse id, built by `cargo xtask build-sources` and loaded with a deploy script. Small works
(confessions, Easton's) could be bundled, but keeping all sources in one place keeps the tool simple.

**The tool.** `commentaries(passage, works?)` returns what each work says on the passage, with the
work's name and tradition. The prompt asks the model to attribute ("Matthew Henry reads this as…"),
never to present a commentary as the meaning of the text.

## Web: reader and Ask

The web app reads the Bible and asks questions, signed in. It doesn't record or capture. It can open
an item read-only, because Ask's citations must open somewhere.

- **UI:** a TypeScript app on Vercel, with HTML and CSS typesetting in Literata, the Convex React
  client and Clerk. egui on the web was rejected for this: no native text selection, find in page or
  screen-reader support, and a large download before first paint.
- **Core on WebAssembly.** The library, Bible search, references and the Ask tools come from
  `linger-core` compiled to `wasm32`, so the web uses the same code as the apps. This needs a
  `native` feature that gates `rusqlite`, `ureq` and the zstd C library, with the pure-Rust `ruzstd`
  decoder for the data files. The data (about 7 MB of translations and 4 MB of original-language
  text) is fetched on first use and kept in the browser's Cache Storage.
- **The tool loop** is ported to TypeScript, about a hundred lines: rounds go to the same proxy, and
  tools run through wasm or `library.search`.

The wasm build is the risky part. Spike it first in phase F, and fall back to a TypeScript port of
the reading and search code only if it fails.

## Plans and costs

Prices are Anthropic list prices through the Gateway, as in the backend plan.

| What | Unit cost |
|---|---|
| Photo, handwriting (Sonnet 5.5: ~1,600 input tokens for the image, ~800 output) | ~$0.011 |
| Photo, slide or printed page (Haiku 5.5) | ~$0.0006 |
| Sermon digest (Haiku 5.5: ~12k in, ~1.5k out) | ~$0.002 |
| Transcription (AssemblyAI, from the backend plan) | ~$0.26 an audio hour |
| Audio storage (AAC 48 kbps) | ~22 MB an audio hour |
| Photo storage | ~0.5 MB with its thumbnail |
| Convex file storage (Professional: 100 GB included) | $0.03 per GB-month |
| Convex file egress, playback and AssemblyAI fetches (50 GB a month included) | $0.12 per GB |

A typical Pro user who records four sermons and takes forty photos a month adds about **$0.25** in
extraction and digests, and about **110 MB** a month of storage, growing over the years they keep
it. At $0.03 per GB-month that is about $0.04 a month after a year of use. Storage is cheap; the
costs to watch are extraction volume and, if added, vector search.

Proposed limits, kept in the `plans` table like the others:

| | Free | Pro |
|---|---|---|
| Library, sync, notes, highlights, Recents | Yes | Yes |
| Photos extracted | 20 a month | 300 a month |
| Sermon audio kept | Transcript only; audio deleted after on-device transcription | Kept |
| Library storage | 1 GB | 25 GB |

Sync was a Pro feature in the backend plan. It can't be now, since the library is the product.

## Privacy, deletion and export

- **The consent sheet changes:**
  - Your library (notes, recordings, photos, transcripts and Ask conversations) is stored in
    Linger's cloud so it is on all your devices.
  - Photos are read by Claude, and recordings transcribed by AssemblyAI. Neither keeps them or trains
    on them.
  - The recording consent stays: the user confirms they may record the speaker.
- **File links.** `ctx.storage.getUrl` links don't expire and need no sign-in: anyone with the link
  can fetch the file until it is deleted. They are unguessable UUIDs, so they are handed only to the
  signed-in owner's apps and to AssemblyAI, and are never logged. If expiring links are needed later,
  Convex's R2 component offers them.
- **Encryption** at rest is Convex's. End-to-end encryption is not offered, because searching and
  extracting need the text on the server. The privacy page says so.
- **Deleting** an item deletes its files and rows (after the 30-day tombstone). **Deleting the
  account** purges every library table and file for the user, from `DELETE /v1/account` and the Clerk
  webhook, as the backend plan already does for its tables.
- **Export** writes Markdown for every item, with its references and sources, plus the original
  photos and audio, as one zip. JSON export of highlights and bookmarks stays.
- **Logs** carry ids, kinds, sizes and timings, never text, as now.

## Build plan

### Phase A: the library in Convex

1. `auth.config.ts`: one `customJwt` provider with the Clerk instance's issuer and JWKS, and no
   `applicationID`. It accepts both the session tokens from iOS and the web and the desktop's OAuth
   access tokens. Both carry the same issuer and the Clerk user id as `sub`, so the same person gets
   the same `tokenIdentifier` on every device. The proxy's `Auth.ts` already relies on the same
   claims. Convex doesn't check `aud` without `applicationID`, and doesn't reject the `at+jwt`
   header (tested). Then the `userQuery` / `userMutation` helpers and `canRead` / `canWrite`.
2. The schema above. Seed the canonical grid from `cargo xtask contract`.
3. Sermons on Convex storage: parts uploaded through upload URLs, the join action, and the
   AssemblyAI step and webhook as a `@convex-dev/workflow` workflow (actions are never retried on
   their own; the workflow retries each step). Audio is kept.
   Remove `Blob.ts`, `DevBlob.ts` and the proxy's `/v1/sermons` routes. Move the existing tests to
   convex-test.
4. Desktop `sync.rs`: subscriptions, the cache in `study.sqlite`, the outbox.
5. **Migration on first sign-in:** existing notes, highlights, bookmarks, Recents and recordings
   (with their transcripts and details) upload once. Local data is kept until the upload is
   confirmed.

*Done when:* a note written on one Mac appears on another within a second, an existing recording
migrates with its transcript, and the app reads and shows cached notes with the network off.

### Phase B: search and Ask

1. Chunking, anchors and the text search index; the digest step for sermons.
2. `library.search`; the `search_library` and `read_item` tools; contract `ask/5`; the prompt rules.
3. Citation chips in the Ask panel, opening an item at its place.
4. The reader's margin marks and the "In your library" list for the chapter.
5. **An eval:** a seeded library (sermons, notes and page photos written for the test, no
   copyrighted text) and 40 questions with the items each answer should cite. `cargo xtask eval-ask`
   reports citation recall and whether answers kept the user's material apart from the text.

*Done when:* the eval cites the expected item in at least 85% of questions, and a question about the
open chapter finds a sermon that cites it only in the digest. If text search misses the bar, add
vectors and run the eval again.

### Phase C: iOS

1. Sign-in, the shared sync engine through `crates/ffi`, the reader with notes and highlights synced.
2. Camera, document scanner, kind picker, PencilKit marks, the outbox.
3. Extraction and the review screen. An extraction eval of 50 photos (handwriting, slides, pages,
   marked and not) decides between Haiku and Sonnet per kind.
4. Recording with AAC, background audio and slides attached by time.

*Done when:* a page marked on the iPhone is answered from on the Mac within a minute of saving, and a
sermon recorded on the iPhone with three slides is cited with its slides.

### Phase D: the Mac as a capture device

1. Continuity Camera ("Take Photo", "Scan Documents"), drag-and-drop and a file picker into the same
   review flow.
2. Live processing state in the library; slide matching to a recording in progress on another
   device.
3. Reading position handoff (`positions` and `NSUserActivity`).

### Phase E: outside sources

1. `cargo xtask build-sources` for the commentaries, Easton's, Nave's and the confessions, with
   provenance and licence notes.
2. Shared tables and the `commentaries` tool.

### Phase F: web

1. The wasm spike: `linger-core` with a `native` feature, `ruzstd`, data from Cache Storage.
2. The reader, ⌘K, search, and Ask with the TypeScript tool loop.
3. Read-only item views for citations.

## Later

- **Church libraries.** A church records a sermon once and its members follow the church. Items gain
  an `ownerId` that is a user or a Clerk organization, and `canRead` admits members. The church holds
  the rights to its own sermons, which also settles the recording question.
- **Sharing one item** with a friend or small group, through the same helper.
- **Re-extraction** of old photos when a better model ships, as an opt-in.

## Open questions

| Question | Who | Blocks |
|---|---|---|
| Does a real token from the Mac app's sign-in get the same `tokenIdentifier` in Convex as one from iOS? Tested with tokens of the same shape, not yet with Clerk's | Sign in on both against a dev deployment | Phase A |
| Convex's docs give actions both a 10- and a 30-minute limit; design to 10 | Convex support | Nothing, if designed to 10 |
| Does AssemblyAI commit in writing to no training on customer audio? | AssemblyAI | Release |
| Is a user's own photo of a copyrighted book, sent to Claude for their private study, acceptable to rely on? | Lawyer | Release |
| Are vectors needed, and which embedding model? | Phase B eval | Phase B |
| Haiku or Sonnet for slides and printed pages | Phase C eval | Phase C |
