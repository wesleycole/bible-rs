/**
 * Cost metering, shared by Convex (settlement) and the proxy. Prices are micro-dollars per million
 * tokens, held on each `models` row so a model switch carries its price with it; the defaults
 * below seed the table from Anthropic's list prices through the Gateway (zero markup).
 */

export interface Prices {
  readonly input: number
  readonly output: number
  readonly cacheRead: number
  readonly cacheWrite: number
}

export interface TokenUsage {
  readonly inputTokens: number
  readonly outputTokens: number
  readonly cacheReadTokens: number
  readonly cacheWriteTokens: number
}

const perMTok = (dollars: number) => Math.round(dollars * 1_000_000)

/** Cache reads cost a tenth of input; five-minute cache writes cost 1.25 times input. */
const priced = (input: number, output: number): Prices => ({
  input: perMTok(input),
  output: perMTok(output),
  cacheRead: perMTok(input * 0.1),
  cacheWrite: perMTok(input * 1.25)
})

export const DEFAULT_PRICES: Record<string, Prices> = {
  "anthropic/claude-haiku-5.5": priced(0.1, 0.5),
  "anthropic/claude-sonnet-5.5": priced(2, 10),
  "anthropic/claude-opus-5.5": priced(4, 20)
}

/** Micro-dollars, rounded up so metering never undercharges. */
export const tokenCost = (usage: TokenUsage, prices: Prices): number =>
  Math.ceil(
    (usage.inputTokens * prices.input +
      usage.outputTokens * prices.output +
      usage.cacheReadTokens * prices.cacheRead +
      usage.cacheWriteTokens * prices.cacheWrite) /
      1_000_000
  )

/**
 * The most one round can cost: every output token the round may write, and its whole input
 * written to the cache (the dearest way to read input).
 */
export const worstCaseRound = (maxTokens: number, estimatedInputTokens: number, prices: Prices): number =>
  tokenCost(
    { inputTokens: 0, outputTokens: maxTokens, cacheReadTokens: 0, cacheWriteTokens: estimatedInputTokens },
    prices
  )

/** Universal-3.5 Pro ($0.21) plus key terms ($0.05), per audio hour. */
export const TRANSCRIPTION_MICROS_PER_HOUR = 260_000

export const transcriptionCost = (seconds: number): number =>
  Math.ceil((seconds * TRANSCRIPTION_MICROS_PER_HOUR) / 3600)
