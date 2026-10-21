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
   one ([backend.md](backend.md#privacy-and-consent)). Waived (owner's decision, October 20): M1b
   ships on AssemblyAI; the Transcriber stays an Effect layer so the provider can change.
2. **Two spikes, two days each:**
   - **The encoder from Rust** (done, October 20): `recording::aac` on `objc2-audio-toolbox` 0.3
     (its `AudioConverter` feature) and `objc2-core-audio-types`, behind core's `apple-audio` feature;
     the bindings cover it, so no C shim. `cargo check --features apple-audio` passes for
     `aarch64-apple-ios` and `aarch64-apple-ios-sim`. One continuous converter per recording, 16 kHz
     mono 16-bit in, 1024-frame `kAudioFormatMPEG4AAC` packets out, bit rate set to 32 kbps (the
     converter offers 12–48 kbps at this rate); the input callback answers a private status code when
     the pushed samples run out, so `push` returns the packets that are complete and keeps the rest
     without padding or copying. Measured in release on an M-series Mac: five minutes → 4690 packets,
     1,025,595 bytes (27.3 kbit/s) in 0.21 s (≈1450× real time); two hours → 112,503 packets,
     27,360,760 bytes (30.4 kbit/s, 13.7 MB an hour) in 4.9 s. Priming is 2112 frames
     (`kAudioConverterPrimeInfo`) and the magic cookie is a whole ES descriptor holding the two-byte
     AudioSpecificConfig `14 08`; variable-size properties must be sized with
     `AudioConverterGetPropertyInfo` first, any other buffer size is refused. The `aarch64-apple-ios`
     release `liblinger_ffi.a` grows by 0.9 MB (79.3 → 80.2 MB, unstripped archive), core's rlib by
     122 KB.
   - **The muxer** (done, October 20): `recording::mp4` writes `ftyp`, `moov`, `mdat`: one track with
     `stsz`/`stts`/`stsc`/`stco`, the `esds` config, an edit list that skips the priming and ends at
     the recorded frame count, and a `roll` sample group (`sgpd`/`sbgp`, roll distance −1). Measured
     on a five-minute and a two-hour file whose every second carries two tones spelling its number
     (the ignored `spike_files` test writes them): `ffprobe` 9.0 reports 300.000000 and 7200.000000
     (start 0, 4690 and 112,503 frames) and a full decode is exactly 4,800,000 and 115,200,000
     samples; AVAsset and AVPlayerItem say 300.0 and 7200.0 (115200000/16000), a zero-tolerance seek
     lands on 3600.0 and AVAssetReader reads second 3600 there; Chrome 145's `<audio>` reports 300 and
     7200, `currentTime = 3600` lands on 3600 and the rendered audio sits on the timeline within
     10 ms; WebKit (an off-screen `WKWebView`, Safari's engine) reports 300 and 7200, seeks to 3600,
     and its `decodeAudioData` gives the exact 115,200,000 samples with second 3600 in place. The bare
     ADTS of the same packets reproduces [library.md](library.md#sermons): `ffprobe` 611.4 s and
     16,260.1 s, AVAsset 10,536.8 s, and the seek "an hour in" plays second 2482. The `roll` group is
     the finding: without it, Apple's decoders (AVAssetReader and AVPlayer, so Safari too) assume
     2112 priming frames on their own and apply the edit list on top, so playback runs 0.132 s early
     and a whole-file read is 2112 samples short; Apple's own encoder writes no edit list at all,
     only an `iTunSMPB` tag, which ffmpeg and Chrome don't read (300.16 s, 0.132 s late). Edit list
     plus `roll` group is what ffmpeg writes, and every player reads it the same way. Chrome's
     `decodeAudioData` alone keeps the 448 frames of end padding; its element and timeline don't.

## linger-core

1. **`recording::aac`** (feature `apple-audio`): one continuous `AudioConverter` per recording, 16 kHz
   mono AAC-LC at the encoder's speech bitrate; PCM buffers in, raw packets out.
2. **`recording::ChunkWriter`** generalizes: ADTS framing of each packet, a new part every 5 minutes at
   a packet boundary, and a packet table (size, offset, part) kept for the muxer. WAV chunks continue on
   the Mac for crash safety and on-device transcription.
3. **`recording::mp4`:** the muxer. One AAC track, `moov` first (`stsz`, `stts`, `stsc`, `stco`, the
   `esds` AudioSpecificConfig, the edit list and `roll` group the spike settled), then `mdat` copied
   from the parts; no re-encoding. Golden files in tests. The spike's `write` takes the packet sizes
   and a reader over the packets; the packet table from 2 feeds it.
   **Built (October 21), where it is smaller:** a part is 4688 packets (the first boundary at or
   after five minutes), and the manifest lists each closed part with its packet count, the
   encoder's priming and the exact sample count; there is no separate packet table, because
   `recording::write_m4a` reads the sizes back from the parts' ADTS headers. If the encoder or a
   part file fails mid-recording, the parts stop (`aac` cleared from the manifest, the error
   logged) and the WAV chunks go on; that recording's audio is encoded from the WAV afterwards,
   as an old recording's is. The golden file is `recording/golden.m4a`, from fixed packets.
4. **Linux:** one FLAC with a seek table at the end through `flacenc`.
5. **Upload sequence**, in core, through the outbox: parts as they close, only if `sermons.begin` said
   `uploadAudio`; at the end, the M4A (or FLAC) in 8 MB `uploadPart` slices, then `sermons.finish`.
   **Built (October 21):** `StudyStore::queue_recording(dir, manifest)` queues three outbox kinds,
   in order: `sermon` (`begin`; its `uploadAudio` answer is kept in a `recordings` table, study
   schema 3), `sermon_part` per closed part (an `audioPart` with its `partIndex`, sent only when
   `begin` said so), and `sermon_end` (the M4A written beside the parts as `audio.m4a`, attached as
   `uploadPart` slices, then `finish` with `audioFileUid`). A file's uid is the SHA-256 of its owner
   and bytes, so two recordings' identical (silent) parts stay two files. The recorder calls it at
   the start, as each part closes and at the end; calling again queues only what is new. A part or
   slice retried after an attach went through is attached once, but its bytes are uploaded again,
   and `files.attach` leaves that second blob in `_storage` unreferenced. No FLAC yet (item 4):
   without AAC parts the end goes without audio.
6. **`transcribe.rs`:** delete the proxy backend and the env-key AssemblyAI backend. Transcripts arrive
   through the change feed as the item's `text` file and are cached as `transcript.json` beside the
   recording, so `transcribe::all` and the details sheet keep working. Development uses a dev Convex
   deployment, like everything else.
   **Built (October 21):** both backends are gone, and with them `linger transcribe` and the
   desktop's transcribe-on-stop, until the desktop half queues its recordings (between those two
   pushes the Mac saves recordings untranscribed). `transcribe::Transcript` is a wrapper: the
   recording's id and start, and `text`, a `records::Transcript`; `transcribe::Segment` is
   `records::Segment`. When a sermon's `text` file is fetched and the sermon was recorded on this
   device, the cache keeps it beside the recording as `transcript.json` (the library's camelCase
   shape); files that earlier builds wrote are still read. A sermon recorded elsewhere isn't
   copied beside a recording (there is none here); Ask reads it once M2 reads transcripts from the
   library.
   **For M5:** `sermon_end` reads the whole M4A into memory and slices copies (tens of MB for two
   hours); fine on the Mac, and sign-out waits on it, which the desktop should show. On iOS the
   slices should stream from the file.
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

**Built (October 20), where it is smaller than the above:** `finish` takes no `partCount`: the
device attaches the slices before it calls `finish`, so `finish` is the word that they are all there,
and the item is `uploading` while the workflow joins them. The join is `library/audio.join({ sermonId })`,
a step of the workflow rather than a function the device calls (a `"use node"` file can hold only
actions, so it is not in `library/files.ts`); it stores the M4A as `audio/mp4` and a dead device's
ADTS parts as `audio/aac`, which is how the item says the timeline is approximate (no new Item
field). The transcript is kept as the item's `text` file, JSON in the shape of `records::Transcript`
(declared once in `crates/core/src/records.rs`, generated into `server/shared/records.ts` like the
records; camelCase, no `recording_id` or `recorded_at`, whose facts live on the item), and the sermon
row keeps no copy. Core's `transcribe::Transcript` becomes an alias or wrapper of `records::Transcript`
in the desktop half, not before. `transcribeNow` is for any `waiting` sermon with audio that
`Allowance.admitSermon` admits now (Pro hours, or a Free trial still unused). The webhook wakes the
workflow with an event; the hourly `check transcription jobs` cron asks the provider about a job
whose webhook is 30 minutes late and wakes it if done, and fails one stuck for 3 hours; the hourly
`finish dead recordings` cron is item 5's first clause. Item 7 is M1's `purgeAccount`, which already
deletes a user's `sermons` rows and every `files` row with its blob; nothing was added. Until item 6
removes the proxy's flow, `blobPath`, `recordingId` and `bytes` stay as optional columns, the
`expire transcripts` cron stays, and `convex/sermons.ts` ignores rows that have an `itemId`. Tests
fake the provider at `fetch`; the Transcriber is the one provider abstraction.

## The desktop app

1. **Recording on macOS** feeds core's encoder from the recorder thread (`apps/desktop/src/recorder/`)
   instead of writing only WAV; the WAV chunks continue.
2. **Recordings:** the details sheet edits the sermon item; transcription status comes from the item
   (`recording`, `uploading`, `waiting`, `processing`, `ready`, `error`) instead of the polling thread. A
   `waiting` sermon says why: on-device transcription arrives in M5, and Pro transcribes in the cloud
   (with "Transcribe now" when hours are available).
3. **Migration:** the M1 sheet gains "Bringing your sermons into your library", with the background
   audio upload shown in the sidebar's sync status.

**Built (October 21), items 1 and 2:** the recorder thread hands PCM to core's `ChunkWriter` and
counts the parts it closes; the app calls `StudyStore::queue_recording` when a recording starts, as
each part closes and when it stops, while sync runs. When sync starts, once the engine has claimed
the cache for the signed-in account (`SyncHandle::claimed`, so an account switch can't forget what
was just queued), `StudyStore::queue_recordings` queues every recording on disk the library doesn't
have. Decisions:
- **The date:** the recorder computes the local date when recording starts and keeps it in the
  manifest as `recorded_on`; core sends it as `recordedOn` and has no time zone. A manifest
  without it falls back to the UTC date of `started_at`.
- **`startedAt`** is sent through the clock offset, like a note's `capturedAt`.
- **Already uploaded:** when `finish` is accepted the manifest records `uploaded` (the owner's
  Clerk id and the `seq`), and the launch queue skips it, so a recording that went to one account
  isn't sent again under the next to sign in here. Recordings from before M1b (no `recorded_on`)
  aren't queued at launch; they come in by migration (item 3).
- **A recording the app died in** (no `ended_at`) is closed at launch from what is on disk: it
  ends at its last closed chunk (`ended_at` = start + its samples), and its M4A holds the AAC
  parts that closed, presenting no more than they hold; the open chunk and part are lost, as a
  crash loses them. Its `finish` then carries this device's duration, rather than the dead-device
  cron's answer two hours later. The recording still being recorded is left open.
- **Status:** About This Recording reads the sermon item's status and says it in a line;
  `waiting` says why (Free: Pro transcribes in the cloud; Pro without hours: they renew; Pro
  with hours: Transcribe Now, which calls `sermons.transcribeNow` through the engine). Saving
  details writes the manifest and the item (`set_sermon_details`: into `begin` if it hasn't gone,
  else an edit of the item). Signing out says when it is waiting on a recording's audio.

## Tests

- **Core:** encoder → packets → M4A has the recording's exact sample count; parts join to the same
  packets the muxer wrote; the muxer's golden file is byte-stable; `ffprobe` on the macOS runner
  reports the exact duration; a recording killed mid-part loses only the open part. `ffprobe` isn't run in
  CI (no ffmpeg on the runners): the spike's four-player measurements on the same `mp4::write`, and
  `afinfo` on an M4A muxed from a recording's parts (727.0208 s, exactly its samples), stand in.
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
