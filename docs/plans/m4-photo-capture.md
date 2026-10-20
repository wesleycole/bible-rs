# M4: Photo capture

Status: not started. Depends on: M3. Design: [library.md](library.md#photos).

**Goal.** Photograph handwritten notes, sermon slides and book pages on the iPhone, mark what matters with
a finger, and have it in Ask on every device: usable within about 30 seconds, and reviewed when the user
gets to it.

**Not in M4:** capture on the Mac (M6), slides attached to a recording by time (M5).

**Release gate.** Book-page capture reaches TestFlight in M4, but public builds keep it behind a release-build
switch (as `LICENSED_ONLINE` does for online translations) until the lawyer has answered the question in [backend.md](backend.md#open-questions). Handwriting and slides
aren't gated.

## Convex

1. **Records:** `sources` joins the synced records ([library.md](library.md#the-data-model)), with
   `library/sources.upsert`. Items gain `pages` (each page's `useWholePage`, `focus`, `printedPage?`
   and `marks`) and `unreviewed`; the pages' `text` and `pageText` are the item's `text` file
   ([library.md](library.md#the-data-model)); `files.role` gains `photo`, `photoMarked` and
   `thumbnail`, with `page`; `chunks` gains `page` and `unreviewed`.
2. **The flow:** `library/photos.begin({ uid, kind, pageCount, sourceUid?, sermonUid?, offsetMs? })`
   creates the item as `uploading`; each page's files attach through `files.attach` (with the storage
   check); `library/photos.submit({ uid, pages: [{ page, marks? }] })` checks every page is
   attached, asks `Allowance` for one photo per page (which checks the month's allowance and the
   per-minute rate limit and counts them in `usage.photos`), makes the item `processing` and starts
   extraction.
3. **Extraction workflow**, one step per page: a Gateway call with `zeroDataRetention`, the
   `extract-handwriting` or `extract-print` model row by kind, the clean `photo` and, for a marked page,
   the `photoMarked` render, and a JSON schema for `{ kind, text, focus: [{ text, mark }], references,
   title?, runningHead?, printedPage? }` ([library.md](library.md#photos)). References are validated
   against the grid. Then:
   - the proposal is written to the item's `text` file, the focus passages to `pages`, and the item
     becomes `review` with `unreviewed: true`;
   - it is chunked provisionally (focus passages as their own chunks), so Ask can use it at once;
   - a ledger row with `kind: extract` per page.

   A failed page becomes an error on the item and gives that page back to the allowance.
4. **What's kept**, per page ([library.md](library.md#photos)): for a marked page, the page's `text` holds
   the marked passages with a sentence of context on each side, and `pageText` the whole page until
   review; for an unmarked page, `text` is the whole page.
5. **`library/photos.approve({ uid, pages: [{ page, text, useWholePage }], references, sourceUid?, title? })`**
   saves the user's corrections, keeps `pageText` only for pages with `useWholePage` (and uses it as that
   page's text), makes the item `ready`, clears `unreviewed`, and re-chunks and re-anchors it. A page set of three or more pages then gets a digest ([library.md](library.md#the-digest-step)).
6. **Allowance**, in the `Allowance` service ([backend.md](backend.md#effect-in-convex)):
   `plans.photosPerMonth` ([backend.md](backend.md#plans-and-limits)), `usage.photos`, `/v1/me`
   `library.photosLeft`, and a rate-limiter bucket (30 photos a minute).
7. **The purge chain** covers `sources`.

## linger-core

- **Marks:** conversion from raw platform strokes (PencilKit's points on iOS, egui's on the Mac in M6)
  to Linger's stroke format (normalized 0–1 points, pen kind, colour), simplification to at most 2,000
  points a page, and **one renderer** (`tiny-skia`) that draws `photoMarked` from a photo and its
  marks. iOS and the Mac both use it, so the same strokes always render the same way.
- **Image preparation** (the core module M6 also uses): decode JPEG and PNG with the `image` crate,
  orient, resize to 2,048 px on the long edge, encode JPEG, make the 400 px thumbnail.
- **`photos::Capture`:** the sequence begin → attach each page's files → submit, driven by core through
  the outbox, so no platform orders it differently; and `approve`.
- **Through the FFI:** raw strokes in, prepared images and renders out, `Capture`, `approve`, and
  sources.

## The iOS app

1. **Capture:** a capture button in the reader and the library. The camera for slides and notes
   (`AVCaptureSession`), the document scanner for pages (`VNDocumentCameraViewController`; a multi-page
   scan is one item), and import from Photos. The kind picker remembers the last choice.
2. **Marks:** a PencilKit canvas over each page, with circle, underline and highlight tools and undo.
   PencilKit's strokes cross the FFI as raw points; core converts them and draws `photoMarked`.
3. **Upload:** Swift hands core each page's captured JPEG bytes; core prepares the 2,048 px page, the
   marked render and the thumbnail and queues them through `Capture`. Works offline; the item shows
   "Waiting to upload".
4. **Review:** each page (pinch to zoom) beside its proposed text with the marked passages highlighted.
   Edit the text, confirm, remove or add references (with the reference picker), choose or create the
   source (book title and author, remembered), and for a marked page choose "Use the whole page" if
   wanted. "Skip review for slides" in Settings.
5. **Item view:** the pages with their marks over them (toggleable), the text, and the anchors as chips
   that open the reader. Unreviewed items say so and offer Review.
6. **Usage descriptions** for the camera and photo library.

## The desktop app

Photos taken on the iPhone appear in the Mac's library, so the desktop gains:
- the photo item view (pages with marks, the text, anchors) and thumbnails in the library list;
- the review screen, for items still in `review` (so a photo taken in church can be reviewed at the desk);
- `{{item … p<n>}}` chips that open the page.

## Evaluation: `cargo xtask eval-extract`

- **50 photos** (`xtask/eval/photos/`): handwriting in several hands, projected slides, printed pages of
  public-domain books, half of them marked (some with a highlighter over the words), with hand-checked
  text, focus passages and references.
- **Scores:** character error rate of `text` and `pageText`, focus recall (each marked passage found),
  reference precision and recall, and cost per photo.
- **It decides** the starting `extract-print` row (Haiku 5.5 unless Sonnet 5.5 is clearly better on slides
  and print) and confirms Sonnet for handwriting ([backend.md](backend.md#model-routing-and-switching)).

## Tests

- convex-test: a marked page's `pageText` is gone after approval unless `useWholePage`; the allowance boundary (the 20th Free photo passes, the 21st is a 402); a three-page scan
  counts three; a failed page gives itself back; approval re-chunks focus passages separately and clears
  `unreviewed`; an invalid reference from the model is dropped; a marked page's `text` holds only the
  marked passages and their context.
- Core: the renderer draws the same pixels for the same strokes on macOS and iOS (golden images).
- iOS: an offline capture uploads when the network returns.

## Done when

- A page marked on the iPhone is usable by Ask on the Mac within 30 seconds of capture, cited as
  unreviewed, and after review is cited with an `{{item …}}` chip that opens the page with its marks.
- The extraction eval meets: character error rate under 3% on print and slides and under 8% on
  handwriting, focus recall at least 90%, reference precision at least 95%.
- Allowances and storage show correctly in Settings on both devices.

## Risks

- **Handwriting quality** varies by hand; review is the safety net, so the review screen must be fast to
  correct on a phone.
- **Provisional chunks** let Ask quote a misread word before review; the tool result marks them, and the
  prompt asks the model to say so.
