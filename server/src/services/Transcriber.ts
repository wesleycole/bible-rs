/**
 * AssemblyAI, with the settings the app's own transcriber uses (crates/core/src/transcribe.rs):
 * Universal-3.5 Pro, speaker labels, and the 66 book names as key terms. Jobs report back to
 * /v1/webhooks/assemblyai with a per-job secret header, and are deleted once their transcript is
 * kept, which deletes AssemblyAI's copy of the audio.
 */
import { Context, Effect, Layer, Redacted, Schema, Stream } from "effect"
import { FetchHttpClient, HttpClient, HttpClientRequest, HttpClientResponse } from "effect/http"
import { UpstreamFailed, UpstreamUnavailable } from "../Errors.js"
import { AppConfig } from "./AppConfig.js"

const API = "https://api.assemblyai.com/v2"

/** The Bible's books by their English names, as `BookId::name` gives them in linger-core. */
export const BOOK_NAMES = [
  "Genesis", "Exodus", "Leviticus", "Numbers", "Deuteronomy", "Joshua", "Judges", "Ruth", "1 Samuel", "2 Samuel",
  "1 Kings", "2 Kings", "1 Chronicles", "2 Chronicles", "Ezra", "Nehemiah", "Esther", "Job", "Psalms", "Proverbs",
  "Ecclesiastes", "Song of Solomon", "Isaiah", "Jeremiah", "Lamentations", "Ezekiel", "Daniel", "Hosea", "Joel",
  "Amos", "Obadiah", "Jonah", "Micah", "Nahum", "Habakkuk", "Zephaniah", "Haggai", "Zechariah", "Malachi",
  "Matthew", "Mark", "Luke", "John", "Acts", "Romans", "1 Corinthians", "2 Corinthians", "Galatians", "Ephesians",
  "Philippians", "Colossians", "1 Thessalonians", "2 Thessalonians", "1 Timothy", "2 Timothy", "Titus", "Philemon",
  "Hebrews", "James", "1 Peter", "2 Peter", "1 John", "2 John", "3 John", "Jude", "Revelation"
] as const

/** The header AssemblyAI echoes on its webhook, carrying the job's secret. */
export const WEBHOOK_HEADER = "x-linger-webhook"

/** One turn of speech, in the shape `transcribe::Segment` saves. */
export const Segment = Schema.Struct({
  start_ms: Schema.Number,
  end_ms: Schema.Number,
  speaker: Schema.NullOr(Schema.String),
  text: Schema.String
})
export type Segment = typeof Segment.Type

const Utterance = Schema.Struct({
  start: Schema.Number,
  end: Schema.Number,
  speaker: Schema.optional(Schema.NullOr(Schema.String)),
  text: Schema.String
})

export const Job = Schema.Struct({
  id: Schema.String,
  status: Schema.Literals(["queued", "processing", "completed", "error"]),
  error: Schema.optional(Schema.NullOr(Schema.String)),
  text: Schema.optional(Schema.NullOr(Schema.String)),
  audio_duration: Schema.optional(Schema.NullOr(Schema.Number)),
  language_code: Schema.optional(Schema.NullOr(Schema.String)),
  utterances: Schema.optional(Schema.NullOr(Schema.Array(Utterance)))
})
export type Job = typeof Job.Type

/** The speaker turns of a finished job; one segment of its whole text when it has none. */
export const segmentsOf = (job: Job): Array<Segment> => {
  const turns = (job.utterances ?? [])
    .map((u) => ({ start_ms: u.start, end_ms: u.end, speaker: u.speaker ?? null, text: u.text.trim() }))
    .filter((s) => s.text.length > 0)
  if (turns.length > 0) return turns
  const text = job.text?.trim()
  return text
    ? [{ start_ms: 0, end_ms: Math.round((job.audio_duration ?? 0) * 1000), speaker: null, text }]
    : []
}

/** Where AssemblyAI gets the audio: a signed URL, or (local development) bytes to upload first. */
export type AudioSource =
  | { readonly _tag: "Url"; readonly url: string }
  | { readonly _tag: "Bytes"; readonly stream: Stream.Stream<Uint8Array, UpstreamFailed> }

export interface TranscriberService {
  /**
   * Starts a job. Without a webhook (a proxy AssemblyAI can't reach, as on a developer's machine)
   * the job is finished when the client next asks for its status.
   */
  readonly start: (input: {
    readonly audio: AudioSource
    readonly webhook: { readonly url: string; readonly secret: string } | undefined
  }) => Effect.Effect<string, UpstreamFailed | UpstreamUnavailable>
  readonly get: (id: string) => Effect.Effect<Job, UpstreamFailed | UpstreamUnavailable>
  /** Deletes the job and AssemblyAI's copy of the audio. Best effort. */
  readonly delete: (id: string) => Effect.Effect<void>
}

const unconfigured = new UpstreamUnavailable({ message: "Cloud transcription isn't configured on this server." })
const failed = (what: string) => new UpstreamFailed({ message: `AssemblyAI failed while ${what}.` })

export class Transcriber extends Context.Service<Transcriber, TranscriberService>()("linger/services/Transcriber") {
  static readonly layer = Layer.effect(
    Transcriber,
    Effect.gen(function*() {
      const { assemblyAi } = yield* AppConfig
      if (!assemblyAi.apiKey) {
        return Transcriber.of({
          start: () => Effect.fail(unconfigured),
          get: () => Effect.fail(unconfigured),
          delete: () => Effect.void
        })
      }
      const key = assemblyAi.apiKey
      const client = (yield* HttpClient.HttpClient).pipe(
        HttpClient.mapRequest((r) =>
          HttpClientRequest.setHeader(HttpClientRequest.prependUrl(r, API), "authorization", Redacted.value(key))
        )
      )
      const ok = HttpClient.filterStatusOk(client)

      const upload = (stream: Stream.Stream<Uint8Array, UpstreamFailed>) =>
        HttpClientRequest.post("/upload").pipe(
          HttpClientRequest.bodyStream(stream, { contentType: "application/octet-stream" }),
          ok.execute,
          Effect.flatMap(HttpClientResponse.schemaBodyJson(Schema.Struct({ upload_url: Schema.String }))),
          Effect.map((r) => r.upload_url),
          Effect.mapError(() => failed("uploading the sermon"))
        )

      const start = Effect.fn("Transcriber.start")(function*(input: {
        readonly audio: AudioSource
        readonly webhook: { readonly url: string; readonly secret: string } | undefined
      }) {
        const audioUrl = input.audio._tag === "Url" ? input.audio.url : yield* upload(input.audio.stream)
        const job = yield* HttpClientRequest.post("/transcript").pipe(
          HttpClientRequest.bodyJsonUnsafe({
            audio_url: audioUrl,
            speech_models: ["universal-3-5-pro", "universal-2"],
            speaker_labels: true,
            punctuate: true,
            format_text: true,
            keyterms_prompt: BOOK_NAMES,
            ...(input.webhook
              ? {
                webhook_url: input.webhook.url,
                webhook_auth_header_name: WEBHOOK_HEADER,
                webhook_auth_header_value: input.webhook.secret
              }
              : {})
          }),
          client.execute,
          Effect.flatMap((response): Effect.Effect<{ readonly id: string }, unknown> =>
            response.status === 200
              ? HttpClientResponse.schemaBodyJson(Schema.Struct({ id: Schema.String }))(response)
              : response.text.pipe(
                Effect.flatMap((text) => Effect.logError("AssemblyAI refused the job", { status: response.status, text })),
                Effect.andThen(Effect.fail(failed("starting the transcription")))
              )
          ),
          Effect.mapError(() => failed("starting the transcription"))
        )
        return job.id
      })

      const get = Effect.fn("Transcriber.get")(function*(id: string) {
        return yield* ok.get(`/transcript/${encodeURIComponent(id)}`).pipe(
          Effect.flatMap(HttpClientResponse.schemaBodyJson(Job)),
          Effect.mapError(() => failed("checking the transcription"))
        )
      })

      const del = (id: string) =>
        client.del(`/transcript/${encodeURIComponent(id)}`).pipe(
          Effect.asVoid,
          Effect.catch(() => Effect.logWarning("couldn't delete the AssemblyAI job"))
        )

      return Transcriber.of({ start, get, delete: del })
    })
  ).pipe(Layer.provide(FetchHttpClient.layer))
}
