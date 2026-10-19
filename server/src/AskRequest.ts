/**
 * Validation of POST /v1/ask. The client sends only the conversation; anything that could turn
 * Linger's account into a general-purpose Claude endpoint is rejected here, before Convex.
 */
import { Effect, Schema } from "effect"
import { type AskContract, contractFor, CONTRACTS } from "./Contract.js"
import { ContractUnsupported, InvalidRequest, TooLarge } from "./Errors.js"

export const MAX_BODY_BYTES = 1_000_000
export const MAX_MESSAGES = 200

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

const TextBlock = Schema.Struct({ type: Schema.Literal("text"), text: Schema.String })
const ToolUseBlock = Schema.Struct({
  type: Schema.Literal("tool_use"),
  id: Schema.String,
  name: Schema.String,
  input: Schema.Unknown
})
const ToolResultContent = Schema.Union([Schema.String, Schema.Array(TextBlock)])
const ToolResultBlock = Schema.Struct({
  type: Schema.Literal("tool_result"),
  tool_use_id: Schema.String,
  content: Schema.optionalKey(ToolResultContent),
  is_error: Schema.optionalKey(Schema.Boolean)
})
const ThinkingBlock = Schema.Struct({
  type: Schema.Literal("thinking"),
  thinking: Schema.String,
  signature: Schema.String
})
const RedactedThinkingBlock = Schema.Struct({ type: Schema.Literal("redacted_thinking"), data: Schema.String })

const Block = Schema.Union([TextBlock, ToolUseBlock, ToolResultBlock, ThinkingBlock, RedactedThinkingBlock])

const Message = Schema.Struct({
  role: Schema.Literals(["user", "assistant"]),
  content: Schema.Union([Schema.String, Schema.NonEmptyArray(Block)])
})

export const AskBody = Schema.Struct({
  contract: Schema.String,
  questionId: Schema.String.check(Schema.isPattern(UUID)),
  round: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
  mode: Schema.Literals(["standard", "deep"]),
  messages: Schema.NonEmptyArray(Message)
})
export type AskBody = typeof AskBody.Type

export interface ValidAsk {
  readonly body: AskBody
  readonly contract: AskContract
  /** The client's messages exactly as sent: they are forwarded byte for byte. */
  readonly messages: ReadonlyArray<unknown>
}

const invalid = (message: string) => new InvalidRequest({ message })

/** Decodes and checks a raw request body. */
export const validateAsk = Effect.fn("validateAsk")(function*(
  text: string
): Effect.fn.Return<ValidAsk, InvalidRequest | TooLarge | ContractUnsupported> {
  if (Buffer.byteLength(text, "utf8") > MAX_BODY_BYTES) {
    return yield* new TooLarge({ message: "This conversation is too long; start a new one." })
  }
  const raw = yield* Effect.try({ try: () => JSON.parse(text) as unknown, catch: () => invalid("The body isn't JSON.") })
  const body = yield* Schema.decodeUnknownEffect(AskBody)(raw).pipe(
    Effect.mapError((e) => invalid(`The request isn't an Ask request: ${e.message}`))
  )
  const contract = contractFor(body.contract)
  if (!contract) {
    return yield* new ContractUnsupported({
      message: `Update Linger to keep using Ask (this proxy serves ${CONTRACTS.map((c) => c.version).join(", ")}).`
    })
  }
  if (body.round >= contract.max_rounds) {
    return yield* invalid(`round must be below ${contract.max_rounds}.`)
  }
  if (body.messages.length > MAX_MESSAGES) {
    return yield* new TooLarge({ message: "This conversation is too long; start a new one." })
  }
  if (body.messages[0].role !== "user" || body.messages[body.messages.length - 1].role !== "user") {
    return yield* invalid("The conversation must start and end with the user.")
  }
  for (let i = 1; i < body.messages.length; i++) {
    if (body.messages[i].role === body.messages[i - 1].role) {
      return yield* invalid("Roles must alternate.")
    }
  }
  const tools = new Set(contract.tools.map((t) => t.name))
  for (const m of body.messages) {
    if (typeof m.content === "string") continue
    for (const block of m.content) {
      if (block.type === "tool_use" && !tools.has(block.name)) {
        return yield* invalid(`Unknown tool ${block.name}.`)
      }
      if (m.role === "user" && (block.type === "tool_use" || block.type === "thinking" || block.type === "redacted_thinking")) {
        return yield* invalid(`A user message can't hold ${block.type}.`)
      }
      if (m.role === "assistant" && block.type === "tool_result") {
        return yield* invalid("An assistant message can't hold a tool result.")
      }
    }
  }
  return { body, contract, messages: (raw as { messages: ReadonlyArray<unknown> }).messages }
})
