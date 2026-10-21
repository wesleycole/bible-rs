/**
 * The digest (docs/plans/library.md, "The digest step"): a model reads a sermon's transcript once
 * and returns the references it read or cited with their times, a short outline and a summary.
 * Through the AI Gateway with zero data retention, as the proxy's Ask calls go, with the `digest`
 * row in `models`. The answer is forced through one tool whose input schema is the digest's
 * shape, so what comes back is JSON of that shape and nothing else.
 */
import { Context, Effect, Layer, Schema } from "effect"
import type { Doc } from "../_generated/dataModel.js"
import { UpstreamFailed, UpstreamUnavailable } from "../../shared/Errors.js"
import type { TokenUsage } from "./pricing.js"

const GATEWAY = "https://ai-gateway.vercel.sh"

export interface Reference {
  readonly ref: string
  readonly atMs?: number
  readonly page?: number
}

export interface DigestOutput {
  readonly references: ReadonlyArray<Reference>
  readonly outline: string
  readonly summary: string
  readonly usage: TokenUsage
}

export interface DigestInput {
  readonly model: Doc<"models">
  readonly title: string
  /** The transcript as the model reads it: one line a turn, each starting with its `[mm:ss]` stamp. */
  readonly text: string
}

const SYSTEM = `You read the transcript of a sermon or talk and report on it for the person who recorded it. \
Each line begins with its [mm:ss] time. Report, through the digest tool:
- references: every Bible passage the speaker reads aloud or cites by name, with the full English book \
name ("Romans 8:28-30", "Psalms 23", "1 Corinthians 13:4-7") and the [mm:ss] time of the line where it \
comes up. Include only passages named in the transcript, not ones you infer. Transcripts of speech have \
mistakes: "Romans eight twenty eight" is Romans 8:28.
- outline: the talk's main points in order, as a short Markdown list.
- summary: one paragraph saying what the talk said, in plain words.`

const TOOL = {
  name: "digest",
  description: "Reports the references, outline and summary of the transcript.",
  input_schema: {
    type: "object",
    properties: {
      references: {
        type: "array",
        items: {
          type: "object",
          properties: {
            ref: { type: "string", description: "The passage, with its full book name: \"Romans 8:28-30\"." },
            at: { type: "string", description: "The [mm:ss] time of the line where it comes up, as \"mm:ss\"." }
          },
          required: ["ref"],
          additionalProperties: false
        }
      },
      outline: { type: "string" },
      summary: { type: "string" }
    },
    required: ["references", "outline", "summary"],
    additionalProperties: false
  }
}

const Message = Schema.Struct({
  content: Schema.Array(Schema.Struct({
    type: Schema.String,
    name: Schema.optional(Schema.String),
    input: Schema.optional(Schema.Unknown)
  })),
  usage: Schema.Struct({
    input_tokens: Schema.Number,
    output_tokens: Schema.Number,
    cache_read_input_tokens: Schema.optional(Schema.NullOr(Schema.Number)),
    cache_creation_input_tokens: Schema.optional(Schema.NullOr(Schema.Number))
  })
})

const Output = Schema.Struct({
  references: Schema.Array(Schema.Struct({ ref: Schema.String, at: Schema.optional(Schema.NullOr(Schema.String)) })),
  outline: Schema.String,
  summary: Schema.String
})

/** "1:02:03", "62:03" or "2:03" as milliseconds; nothing for anything else. */
export const clockMs = (at: string): number | undefined => {
  const parts = at.trim().split(":").map(Number)
  if (parts.length < 2 || parts.length > 3 || parts.some((n) => !Number.isInteger(n) || n < 0)) return undefined
  const [h, m, s] = parts.length === 3 ? parts : [0, ...parts]
  return ((h! * 60 + m!) * 60 + s!) * 1000
}

export class Digest extends Context.Service<Digest, {
  readonly digest: (input: DigestInput) => Effect.Effect<DigestOutput, UpstreamFailed | UpstreamUnavailable>
}>()("linger/convex/Digest") {
  static readonly layer = Layer.sync(Digest, () => {
    const failed = new UpstreamFailed({ message: "The digest failed." })
    const digest = Effect.fn("Digest.digest")(function*(input: DigestInput) {
      const key = process.env.AI_GATEWAY_API_KEY
      if (!key) return yield* new UpstreamUnavailable({ message: "The digest isn't configured on this deployment." })
      const url = process.env.AI_GATEWAY_URL ?? GATEWAY
      const response = yield* Effect.tryPromise({
        try: () =>
          fetch(`${url}/v1/messages`, {
            method: "POST",
            headers: { "x-api-key": key, "anthropic-version": "2023-06-01", "content-type": "application/json" },
            body: JSON.stringify({
              model: input.model.gatewayModel,
              max_tokens: input.model.maxTokens,
              thinking: { type: "disabled" },
              system: SYSTEM,
              tools: [TOOL],
              tool_choice: { type: "tool", name: TOOL.name },
              messages: [{ role: "user", content: `Title: ${input.title || "(untitled)"}\n\n${input.text}` }],
              providerOptions: { gateway: { zeroDataRetention: true } }
            })
          }),
        catch: () => failed
      })
      if (!response.ok) {
        yield* Effect.logError("the Gateway refused the digest", { status: response.status, text: yield* Effect.promise(() => response.text()) })
        return yield* failed
      }
      const body = yield* Effect.tryPromise({ try: () => response.json(), catch: () => failed })
      const message = yield* Schema.decodeUnknownEffect(Message)(body).pipe(Effect.mapError(() => failed))
      const call = message.content.find((c) => c.type === "tool_use" && c.name === TOOL.name)
      const output = yield* Schema.decodeUnknownEffect(Output)(call?.input).pipe(Effect.mapError(() => failed))
      return {
        references: output.references.map((r) => {
          const atMs = r.at ? clockMs(r.at) : undefined
          return { ref: r.ref, ...(atMs !== undefined ? { atMs } : {}) }
        }),
        outline: output.outline.trim(),
        summary: output.summary.trim(),
        usage: {
          inputTokens: message.usage.input_tokens,
          outputTokens: message.usage.output_tokens,
          cacheReadTokens: message.usage.cache_read_input_tokens ?? 0,
          cacheWriteTokens: message.usage.cache_creation_input_tokens ?? 0
        }
      }
    })
    return Digest.of({ digest })
  })
}
