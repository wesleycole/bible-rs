/**
 * The transcription provider behind one service, so it can change. AssemblyAI is the first
 * implementation, with the settings the app's own transcriber uses (crates/core/src/transcribe.rs):
 * Universal-3.5 Pro, speaker labels and the 66 book names as key terms. A job reports back to
 * `POST /assemblyai` (convex/http.ts) with a per-job secret header, and is deleted once its
 * transcript is kept, which deletes the provider's copy of the audio.
 */
import { Context, Effect, Layer, Schema } from "effect"
import { UpstreamFailed, UpstreamUnavailable } from "../../shared/Errors.js"
import type { Transcript } from "../../shared/records.js"

const API = "https://api.assemblyai.com/v2"

/** The Bible's books by their English names, as `BookId::name` gives them in linger-core. */
const BOOK_NAMES = [
  "Genesis", "Exodus", "Leviticus", "Numbers", "Deuteronomy", "Joshua", "Judges", "Ruth", "1 Samuel", "2 Samuel",
  "1 Kings", "2 Kings", "1 Chronicles", "2 Chronicles", "Ezra", "Nehemiah", "Esther", "Job", "Psalms", "Proverbs",
  "Ecclesiastes", "Song of Solomon", "Isaiah", "Jeremiah", "Lamentations", "Ezekiel", "Daniel", "Hosea", "Joel",
  "Amos", "Obadiah", "Jonah", "Micah", "Nahum", "Habakkuk", "Zephaniah", "Haggai", "Zechariah", "Malachi",
  "Matthew", "Mark", "Luke", "John", "Acts", "Romans", "1 Corinthians", "2 Corinthians", "Galatians", "Ephesians",
  "Philippians", "Colossians", "1 Thessalonians", "2 Thessalonians", "1 Timothy", "2 Timothy", "Titus", "Philemon",
  "Hebrews", "James", "1 Peter", "2 Peter", "1 John", "2 John", "3 John", "Jude", "Revelation"
]

/** The header the provider echoes on its webhook, carrying the job's secret. */
export const WEBHOOK_HEADER = "x-linger-webhook"

const Job = Schema.Struct({
  id: Schema.String,
  status: Schema.Literals(["queued", "processing", "completed", "error"]),
  error: Schema.optional(Schema.NullOr(Schema.String)),
  text: Schema.optional(Schema.NullOr(Schema.String)),
  audio_duration: Schema.optional(Schema.NullOr(Schema.Number)),
  language_code: Schema.optional(Schema.NullOr(Schema.String)),
  utterances: Schema.optional(Schema.NullOr(Schema.Array(Schema.Struct({
    start: Schema.Number,
    end: Schema.Number,
    speaker: Schema.optional(Schema.NullOr(Schema.String)),
    text: Schema.String
  }))))
})
type Job = typeof Job.Type

/** A finished job's transcript (the item's `text` file, `records::Transcript`): its speaker turns, or one segment of its whole text when it has none. */
const transcriptOf = (job: Job): Transcript => {
  const durationMs = Math.round((job.audio_duration ?? 0) * 1000)
  const turns = (job.utterances ?? [])
    .map((u) => ({ startMs: u.start, endMs: u.end, ...(u.speaker ? { speaker: u.speaker } : {}), text: u.text.trim() }))
    .filter((s) => s.text.length > 0)
  const text = job.text?.trim()
  const segments = turns.length > 0 ? turns : text ? [{ startMs: 0, endMs: durationMs, text }] : []
  return { durationMs, ...(job.language_code ? { language: job.language_code } : {}), segments }
}

export type JobState =
  | { readonly status: "pending" }
  | { readonly status: "completed"; readonly transcript: Transcript }
  | { readonly status: "error"; readonly error: string }

export class Transcriber extends Context.Service<Transcriber, {
  /** Starts a job on the audio at `audioUrl`; the provider POSTs to `webhook.url` with `webhook.secret` in WEBHOOK_HEADER when it is done. */
  readonly start: (input: {
    readonly audioUrl: string
    readonly webhook: { readonly url: string; readonly secret: string }
  }) => Effect.Effect<string, UpstreamFailed | UpstreamUnavailable>
  readonly get: (id: string) => Effect.Effect<JobState, UpstreamFailed | UpstreamUnavailable>
  /** Deletes the job and the provider's copy of the audio. Best effort. */
  readonly delete: (id: string) => Effect.Effect<void>
}>()("linger/convex/Transcriber") {
  static readonly layer = Layer.sync(Transcriber, () => {
    const failed = (what: string) => new UpstreamFailed({ message: `AssemblyAI failed while ${what}.` })
    const call = (what: string, path: string, init: RequestInit) =>
      Effect.gen(function*() {
        const key = process.env.ASSEMBLYAI_API_KEY
        if (!key) return yield* new UpstreamUnavailable({ message: "Cloud transcription isn't configured on this deployment." })
        const response = yield* Effect.tryPromise({
          try: () => fetch(`${API}${path}`, { ...init, headers: { ...(init.headers ?? {}), authorization: key, "content-type": "application/json" } }),
          catch: () => failed(what)
        })
        if (!response.ok) {
          yield* Effect.logError("AssemblyAI refused", { what, status: response.status, text: yield* Effect.promise(() => response.text()) })
          return yield* failed(what)
        }
        return yield* Effect.tryPromise({ try: () => response.json(), catch: () => failed(what) })
      })

    const start = Effect.fn("Transcriber.start")(function*(input: {
      readonly audioUrl: string
      readonly webhook: { readonly url: string; readonly secret: string }
    }) {
      const body = yield* call("starting the transcription", "/transcript", {
        method: "POST",
        body: JSON.stringify({
          audio_url: input.audioUrl,
          speech_models: ["universal-3-5-pro", "universal-2"],
          speaker_labels: true,
          punctuate: true,
          format_text: true,
          keyterms_prompt: BOOK_NAMES,
          webhook_url: input.webhook.url,
          webhook_auth_header_name: WEBHOOK_HEADER,
          webhook_auth_header_value: input.webhook.secret
        })
      })
      const job = yield* Schema.decodeUnknownEffect(Schema.Struct({ id: Schema.String }))(body).pipe(
        Effect.mapError(() => failed("starting the transcription"))
      )
      return job.id
    })

    const get = Effect.fn("Transcriber.get")(function*(id: string): Effect.fn.Return<JobState, UpstreamFailed | UpstreamUnavailable> {
      const body = yield* call("checking the transcription", `/transcript/${encodeURIComponent(id)}`, { method: "GET" })
      const job = yield* Schema.decodeUnknownEffect(Job)(body).pipe(Effect.mapError(() => failed("checking the transcription")))
      switch (job.status) {
        case "completed":
          return { status: "completed", transcript: transcriptOf(job) }
        case "error":
          return { status: "error", error: job.error ?? "The transcription failed." }
        default:
          return { status: "pending" }
      }
    })

    const del = (id: string) =>
      call("deleting the job", `/transcript/${encodeURIComponent(id)}`, { method: "DELETE" }).pipe(
        Effect.asVoid,
        Effect.catch(() => Effect.logWarning("couldn't delete the AssemblyAI job", { id }))
      )

    return Transcriber.of({ start, get, delete: del })
  })
}
