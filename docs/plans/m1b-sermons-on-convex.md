# M1b: Sermons on Convex

Status: not started; M3a (iOS on M1's surface) runs first. Depends on: M1. Design: [library.md](library.md#sermons),
[backend.md](backend.md#state-in-convex).

**Goal.** Sermons are library items: recorded through one encoder in core, written to one M4A on the
device, stored and transcribed entirely in Convex, and playable on every device for Pro. Vercel Blob is
gone, and so is every sermon path in the proxy.

**Not in M1b:** iOS recording (M5, which reuses everything here through the FFI), on-device
transcription (M5), the digest (M2), playback of synced audio in the desktop app (M2).

## Before any code ships

1. **AssemblyAI's written no-training commitment**, or a move to a transcription provider that gives
   one ([backend.md](backend.md#privacy-and-consent)).
2. **Two spikes, two days each:**
   - **The encoder from Rust:** `AudioConverter` through `objc2-audio-toolbox` (or `coreaudio-sys`)
     encoding a 5-minute 16 kHz WAV to AAC-LC packets on macOS, built for an iOS target too. If the
     bindings fall short, a C shim built by `build.rs`, not Swift.
   - **The muxer:** `recording::mp4` writes an M4A from those packets; AVPlayer, Safari, Chrome and
     `ffprobe` report its exact duration and seek exactly, and a timestamp an hour into a two-hour test
     file lands on its sentence. The ADTS numbers in [library.md](library.md#sermons) are why this
     matters.

## linger-core

1. **`recording::aac`** (feature `apple-audio`): one continuous `AudioConverter` per recording, 16 kHz
   mono AAC-LC at the encoder's speech bitrate; PCM buffers in, raw packets out.
2. **`recording::ChunkWriter`** generalizes: ADTS framing of each packet, a new part every 5 minutes at
   a packet boundary, and a packet table (size, offset, part) kept for the muxer. WAV chunks continue on
   the Mac for crash safety and on-device transcription.
3. **`recording::mp4`:** the muxer. One AAC track, `moov` first (`stsz`, `stts`, `stsc`, `stco`, the
   `esds` AudioSpecificConfig), then `mdat` copied from the parts; no re-encoding. Golden files in
   tests.
4. **Linux:** one FLAC with a seek table at the end through `flacenc`.
5. **Upload sequence**, in core, through the outbox: parts as they close, only if `sermons.begin` said
   `uploadAudio`; at the end, the M4A (or FLAC) in 8 MB `uploadPart` slices, then `sermons.finish`.
6. **`transcribe.rs`:** delete the proxy backend and the env-key AssemblyAI backend. Transcripts arrive
   through the change feed as the item's `text` file and are cached as `transcript.json` beside the
   recording, so `transcribe::all` and the details sheet keep working. Development uses a dev Convex
   deployment, like everything else.
7. **Migration of recordings**: details and transcript first (the recording's UUID becomes the item's
   `uid`, the transcript its `text` file), so they're searchable at once; then, for Pro users only, the
   audio encoded from the WAV chunks and muxed in the background, uploaded in slices. Free users' audio
   stays on the device.

## Convex

1. **Schema:** `sermons` gains `itemId` and `audio` and loses `blobPath` and the 24-hour expiry; it stays
   the transcription job ([library.md](library.md#the-data-model)).
2. **Functions** (Effect handlers, [backend.md](backend.md#effect-in-convex)):
   - `library/sermons.begin({ uid, details, recordedOn })` creates the item as `recording` and the job,
     and answers `{ uploadAudio, cloudTranscript }` from `Allowance`.
   - `library/sermons.finish({ uid, durationMs, partCount, audioFileUid? })` moves the item to
     `uploading` until every part and the M4A's slices are attached. Without `cloudTranscript` the item
     becomes `waiting`. Otherwise it reserves the duration through `Allowance` and starts the workflow.
   - `library/sermons.transcribeNow({ uid })` starts the same workflow for a `waiting` Pro sermon once
     hours are available.
   - `library/files.join({ uid })`, a Node action for its memory: concatenates `uploadPart` slices byte
     for byte into one file, verifies the SHA-256 the client sent, and deletes the slices.
3. **The workflow** (`@convex-dev/workflow`): join → start AssemblyAI on the file's `getUrl` link → wait
   for the webhook → store the transcript as the `text` file → settle the real duration through
   `Allowance` → delete AssemblyAI's job → delete the parts → mark the item `ready` → delete a Free
   trial sermon's audio.
4. **`convex/http.ts`:** `POST /assemblyai` checks the per-job secret header against its stored hash and
   resumes the workflow.
5. **Crons:** finish a recording with no new part for two hours and no `finish`, concatenating the ADTS
   parts it has into the `audio` file and marking its timeline approximate; check on jobs whose webhook
   is late; fail jobs stuck over 3 hours and give back the trial hour.
6. **Remove from the proxy:** `services/Blob.ts`, `services/Transcriber.ts`, `routes/DevBlob.ts`,
   `SermonFlow.ts`, the `/v1/sermons` handlers (the routes answer `409` for one release; see Risks),
   `/v1/webhooks/assemblyai`, `/v1/internal/cleanup`, the `sermons.expire` cron, and the Blob and
   AssemblyAI variables. Disconnect the `linger-sermons` Blob store after the deploy.
7. **The purge chain** covers sermon jobs and every audio file.

## The desktop app

1. **Recording on macOS** feeds core's encoder from the recorder thread (`apps/desktop/src/recorder/`)
   instead of writing only WAV; the WAV chunks continue.
2. **Recordings:** the details sheet edits the sermon item; transcription status comes from the item
   (`recording`, `uploading`, `waiting`, `processing`, `ready`, `error`) instead of the polling thread. A
   `waiting` sermon says why: on-device transcription arrives in M5, and Pro transcribes in the cloud
   (with "Transcribe now" when hours are available).
3. **Migration:** the M1 sheet gains "Bringing your sermons into your library", with the background
   audio upload shown in the sidebar's sync status.

## Tests

- **Core:** encoder → packets → M4A has the recording's exact sample count; parts join to the same
  packets the muxer wrote; the muxer's golden file is byte-stable; `ffprobe` on the macOS runner
  reports the exact duration; a recording killed mid-part loses only the open part.
- **convex-test:** `begin` answers `uploadAudio: false` for a Free user past the trial; the workflow
  settles the real duration, gives back the trial hour on failure, deletes trial audio and the parts,
  and resumes from a late webhook through the cron; the dead-device cron finishes with the parts it has
  and marks the timeline approximate; `files.join` is byte-exact and rejects a bad SHA-256.
- **Desktop:** a one-hour recording's parts upload as it records (watched in the dev deployment).

## Done when

- A one-hour sermon recorded on the Mac by a Pro user uploads as it records, finishes as one M4A, is
  transcribed, plays back from Convex with exact seeking (checked with `curl` range requests and
  AVPlayer), and the allowance moves by its real duration. A Free user's recording past the trial
  uploads no audio.
- Old recordings migrate: transcripts are searchable at once, and a Pro user's audio follows.
- Nothing is left in Vercel Blob, and the proxy has no Blob or AssemblyAI code.
- There is one encoder, one framer and one muxer, all in core, and the iOS build of core compiles them.

## Risks

- **AudioToolbox from Rust.** Binding coverage decides whether the encoder is a few hundred lines or
  needs a C shim; the spike settles it before anything else is written.
- **Muxer correctness across players.** A wrong `stts` or `esds` plays in one player and not another;
  the spike tests four.
- **Old desktop builds** call `/v1/sermons`. For one release those routes stay, answering
  `409 contract_unsupported`, so old builds say "Update Linger" instead of failing oddly; then they go.
