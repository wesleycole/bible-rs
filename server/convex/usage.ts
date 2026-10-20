/**
 * Ask metering, as the proxy calls it: beginning and settling a model round, and the sweep that
 * charges rounds that never settled. The rules are `Allowance`'s; these are its doors.
 */
import { v } from "convex/values"
import { Effect } from "effect"
import type { Doc, Id } from "./_generated/dataModel.js"
import { ensureUser } from "./lib/accounts.js"
import { effectInternalMutation, effectServerMutation } from "./lib/effect.js"
import { Allowance } from "./lib/allowance.js"
import { denial, type Denial } from "./lib/errors.js"
import { MutationCtx } from "./lib/services.js"

export type BeginRound =
  | {
      ok: true
      reservationId: Id<"reservations">
      model: Doc<"questions">["pinned"]
      maxRounds: number
      questionsLeft: number
      downgraded: boolean
    }
  | Denial

export const beginRound = effectServerMutation({
  args: {
    clerkId: v.string(),
    email: v.optional(v.string()),
    questionId: v.string(),
    round: v.number(),
    mode: v.union(v.literal("standard"), v.literal("deep")),
    estimatedInputTokens: v.number()
  },
  handler: Effect.fn("usage.beginRound")(function*(args) {
    const ctx = yield* MutationCtx
    const user = yield* Effect.promise(() => ensureUser(ctx, args.clerkId, args.email))
    return yield* (yield* Allowance).beginRound(user, args).pipe(
      Effect.map((granted): BeginRound => ({ ok: true, ...granted })),
      // A denial is an answer the proxy maps onto its error contract, not a thrown error.
      Effect.catch((e) => Effect.succeed<BeginRound>(denial(e)))
    )
  })
})

const tokens = v.object({
  inputTokens: v.number(),
  outputTokens: v.number(),
  cacheReadTokens: v.number(),
  cacheWriteTokens: v.number()
})

/**
 * Replaces a round's reservation with what it cost. Without usage (the stream ended before its
 * final usage arrived), the reservation itself is charged. Settling twice is a no-op.
 */
export const settleRound = effectServerMutation({
  args: {
    reservationId: v.id("reservations"),
    usage: v.optional(tokens),
    status: v.string(),
    ttftMs: v.optional(v.number()),
    durationMs: v.number()
  },
  handler: Effect.fn("usage.settleRound")(function*(args) {
    return yield* (yield* Allowance).settleRound(args)
  })
})

/** Crash recovery: Ask reservations that never settled are charged in full after 15 minutes. */
export const sweepReservations = effectInternalMutation({
  args: {},
  handler: Effect.fn("usage.sweepReservations")(function*() {
    return yield* (yield* Allowance).sweep
  })
})
