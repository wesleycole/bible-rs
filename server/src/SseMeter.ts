/**
 * Reads an Anthropic Messages event stream as it passes through, for metering: the usage the
 * final `message_delta` reports and when the first token arrived. The bytes themselves go to the
 * client untouched.
 */
import type { TokenUsage } from "../convex/lib/pricing.js"

interface RawUsage {
  readonly input_tokens?: number
  readonly output_tokens?: number
  readonly cache_read_input_tokens?: number
  readonly cache_creation_input_tokens?: number
}

const n = (v: unknown): number | undefined => (typeof v === "number" && Number.isFinite(v) ? v : undefined)

export class SseMeter {
  private readonly decoder = new TextDecoder()
  private buffer = ""
  private usage: { input?: number; output?: number; cacheRead?: number; cacheWrite?: number } = {}
  /** The final usage arrived (`message_delta` with usage). */
  final = false
  /** The stream finished normally (`message_stop`). */
  stopped = false
  /** An `error` event came through. */
  errored = false
  firstTokenAt: number | undefined

  private readonly now: () => number

  constructor(now: () => number = Date.now) {
    this.now = now
  }

  push(chunk: Uint8Array): void {
    this.buffer += this.decoder.decode(chunk, { stream: true })
    let end: number
    // Events end with a blank line; tolerate CRLF.
    while ((end = this.buffer.search(/\r?\n\r?\n/)) >= 0) {
      const raw = this.buffer.slice(0, end)
      const sep = this.buffer.slice(end).match(/^\r?\n\r?\n/)![0].length
      this.buffer = this.buffer.slice(end + sep)
      this.event(raw)
    }
  }

  private event(raw: string): void {
    const data = raw
      .split(/\r?\n/)
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trimStart())
      .join("\n")
    if (!data) return
    let e: Record<string, any>
    try {
      e = JSON.parse(data)
    } catch {
      return
    }
    switch (e.type) {
      case "message_start":
        this.merge(e.message?.usage)
        break
      case "content_block_start":
      case "content_block_delta":
        this.firstTokenAt ??= this.now()
        break
      case "message_delta":
        this.merge(e.usage)
        // The Gateway reports cache reads and writes in the provider's own usage.
        this.merge(e.provider_metadata?.anthropic?.usage)
        if (e.usage) this.final = true
        break
      case "message_stop":
        this.stopped = true
        break
      case "error":
        this.errored = true
        break
    }
  }

  /** Later reports are cumulative, so the largest count seen is the count. */
  private merge(u: RawUsage | undefined): void {
    if (!u) return
    const max = (a: number | undefined, b: unknown) => {
      const v = n(b)
      return v === undefined ? a : Math.max(a ?? 0, v)
    }
    this.usage = {
      input: max(this.usage.input, u.input_tokens),
      output: max(this.usage.output, u.output_tokens),
      cacheRead: max(this.usage.cacheRead, u.cache_read_input_tokens),
      cacheWrite: max(this.usage.cacheWrite, u.cache_creation_input_tokens)
    }
  }

  /** The round's usage, once the final report arrived. */
  result(): TokenUsage | undefined {
    if (!this.final) return undefined
    return {
      inputTokens: this.usage.input ?? 0,
      outputTokens: this.usage.output ?? 0,
      cacheReadTokens: this.usage.cacheRead ?? 0,
      cacheWriteTokens: this.usage.cacheWrite ?? 0
    }
  }
}

/** An Anthropic `error` event, for failures after the stream has started. */
export const errorEvent = (message: string, type = "api_error"): Uint8Array =>
  new TextEncoder().encode(`event: error\ndata: ${JSON.stringify({ type: "error", error: { type, message } })}\n\n`)
