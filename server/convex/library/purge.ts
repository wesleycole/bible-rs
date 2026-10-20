/**
 * The scheduled links of `lib/purge.ts`: an account's chain, and the daily cron's tombstone and
 * orphaned-blob sweeps. Each link does one step and schedules the next while more remain.
 */
import { v } from "convex/values"
import { Clock, Effect } from "effect"
import { internal } from "../_generated/api.js"
import type { Id } from "../_generated/dataModel.js"
import { effectInternalMutation } from "../lib/effect.js"
import { Purge } from "../lib/purge.js"
import { MutationCtx } from "../lib/services.js"
import { DAY } from "../lib/time.js"

/** The orphan sweep looks this far back each day, so every blob is examined in two runs. */
const ORPHAN_WINDOW = 3 * DAY

/** One step of an account's purge, scheduling the next while records remain. */
export const purgeAccount = Effect.fnUntraced(function*(userId: Id<"users">): Effect.fn.Return<{ done: boolean }, never, Purge | MutationCtx> {
  const { done } = yield* (yield* Purge).account(userId)
  if (!done) {
    const ctx = yield* MutationCtx
    yield* Effect.promise(() => ctx.scheduler.runAfter(0, internal.library.purge.step, { userId }))
  }
  return { done }
})

export const step = effectInternalMutation({
  args: { userId: v.id("users") },
  handler: Effect.fn("purge.step")(function*({ userId }): Effect.fn.Return<{ done: boolean }, never, Purge | MutationCtx> {
    return yield* purgeAccount(userId)
  })
})

export const tombstones = effectInternalMutation({
  args: {},
  handler: Effect.fn("purge.tombstones")(function*(): Effect.fn.Return<{ deleted: number }, never, Purge | MutationCtx> {
    const { deleted, more } = yield* (yield* Purge).tombstones
    if (more) {
      const ctx = yield* MutationCtx
      yield* Effect.promise(() => ctx.scheduler.runAfter(0, internal.library.purge.tombstones, {}))
    }
    return { deleted }
  })
})

/** Deletes blobs nothing points at; `from` (a creation time) continues a sweep. */
export const orphans = effectInternalMutation({
  args: { from: v.optional(v.number()) },
  handler: Effect.fn("purge.orphans")(function*({ from }): Effect.fn.Return<{ deleted: number }, never, Purge | MutationCtx> {
    const start = from ?? (yield* Clock.currentTimeMillis) - ORPHAN_WINDOW
    const { deleted, next } = yield* (yield* Purge).orphans(start)
    if (next !== undefined) {
      const ctx = yield* MutationCtx
      yield* Effect.promise(() => ctx.scheduler.runAfter(0, internal.library.purge.orphans, { from: next }))
    }
    return { deleted }
  })
})
