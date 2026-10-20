# M5: Recording everywhere

Status: not started. Depends on: M1b, M3, M4. Design: [library.md](library.md#sermons).

**Goal.** Record a sermon on the iPhone in the pew, photograph the slides as they come up, and have the
sermon, its transcript and its slides together in the library. Free users get transcripts on the
device on iOS 26, every supported macOS and Linux.

## The iOS app

1. **Recording:** `AVAudioEngine` from the microphone, the `audio` background mode, and the audio
   session set for recording. Interruptions (calls, Siri, another app's audio) pause and resume
   without losing the part in progress; a route change (AirPods) keeps recording.
2. **Encoding:** the `AVAudioEngine` tap hands 16 kHz mono PCM buffers across the FFI, and core's
   encoder, framer, part writer and muxer from M1b do the rest ([library.md](library.md#sermons)): there
   is no iOS encoder. Parts go to the outbox as they close, only if `sermons.begin` said `uploadAudio`,
   and the M4A is written and uploaded at the end, as on the Mac.
3. **A Live Activity** (ActivityKit) with the elapsed time and a stop button, and a recording
   indicator in the app.
4. **Details:** title, speaker and church, prefilled from the last recording, as the Mac's details
   sheet does; the recording consent at the first recording
   ([backend.md](backend.md#privacy-and-consent)).
5. **Slides and sermons** ([library.md](library.md#sermons)):
   - The capture button stays available while recording; a photo taken then attaches to the sermon with
     `offsetMs`.
   - If no recording is running here but a sermon is in `recording` status on another of the user's
     devices, the capture screen offers "Attach to the sermon recording on your Mac". Both times are in
     server time (the sync engine's clock offset), so `offsetMs` is exact to a second or two.
   - Any slide can be attached to a sermon, or moved, afterwards from its page.
   - **Sermon notes** without a recording: a sermon item with no audio, for slides and handwritten notes
     from one service.
6. **Usage description** for the microphone.

## Convex

- `library/sermons.begin` accepts `recordedOn: "ios"`, and `library/sermons.createNotes({ uid, details })`
  makes a sermon item with no audio. `photos.begin` accepts `sermonUid` and `offsetMs` (from M4), and the
  sermon's `read_item` output places slides inline at their times.
- **Transcripts from the device:** `library/sermons.attachTranscript({ uid, transcript, engine })` for
  on-device transcripts. No reservation; the item records the engine, and the digest, chunking and
  anchoring run as for cloud transcripts. Free audio isn't uploaded ([plans](backend.md#plans-and-limits)).

## On-device transcription

- **iOS 26:** Apple's `SpeechTranscriber` (the Speech framework's long-form transcriber) on the recorded
  audio after the recording ends, with the 66 book names as contextual strings. iOS 17 and 18 get the
  cloud trial hour only, and the Pro sheet says so.
- **macOS 26:** the same through one small Swift static library with a C ABI (`apps/desktop/swift/`,
  built by `apps/desktop/build.rs` with `swiftc` on macOS only, and linked into the iOS app too), called
  from the transcription thread. It takes a file path and the contextual strings and returns the
  transcript shape, nothing else.
- **macOS 11–15 and Linux:** `whisper-rs` with the `base.en` model (Metal on Apple silicon), downloaded
  on first use (about 140 MB) with progress and a checksum, stored in `<data>/models/`.
- Output becomes the same `{ language, durationMs, segments }` shape. The item notes the transcript was
  made on the device, and Pro users can send it to the cloud for a better one.
- Free sermons over the trial hour use this automatically; Pro users can choose it in Settings for
  privacy or when offline.

## Tests

- iOS: a 30-minute recording through a simulated phone call keeps every part, and the M4A has the
  exact duration; a slide taken at 12:30 appears in `read_item` at 12:30.
- Mac and Linux: on-device transcription of a public-domain reading (a LibriVox recording of a gospel)
  has a word error rate under 15% (`cargo xtask eval-transcribe`).
- convex-test: `attachTranscript` doesn't touch allowances; a Free user's sermon past the trial never stores audio; a
  `waiting` sermon is transcribed when its on-device transcript arrives.

## Done when

- A one-hour sermon recorded on the iPhone, with three slides photographed during it, is in the library
  with its transcript and slides, and Ask cites a slide's point with the time it was shown.
- A slide taken on the iPhone while the Mac records attaches to the Mac's sermon.
- A Free user's sermon is transcribed on the device on iOS 26, every supported macOS and Linux, and its
  audio stays on that device.
- A slide photographed with no recording is attached to a sermon afterwards, and "Sermon notes" gathers
  a service's slides without audio.

## Risks

- **Background recording** is the most fragile part on iOS: test long recordings on a locked phone,
  with Low Power Mode, and with the app killed by memory pressure (the parts already uploaded survive).
- **SpeechTranscriber** quality on preaching (one speaker, a large room) is unmeasured; the eval above
  measures it before Free users rely on it.
