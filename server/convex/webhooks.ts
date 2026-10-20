import { v } from "convex/values"
import { Clock, Effect } from "effect"
import { effectInternalMutation, effectServerMutation } from "./lib/effect.js"
import { MutationCtx } from "./lib/services.js"
import { DAY } from "./lib/time.js"

const find = Effect.fnUntraced(function*(source: string, eventId: string) {
  const ctx = yield* MutationCtx
  return yield* Effect.promise(() =>
    ctx.db.query("webhookEvents").withIndex("by_source_event", (q) => q.eq("source", source).eq("eventId", eventId)).unique()
  )
})

/** Records a webhook delivery; `fresh` is false when this event was already handled. */
export const claim = effectServerMutation({
  args: { source: v.string(), eventId: v.string() },
  handler: Effect.fn("webhooks.claim")(function*({ source, eventId }) {
    if (yield* find(source, eventId)) return { fresh: false }
    const ctx = yield* MutationCtx
    const receivedAt = yield* Clock.currentTimeMillis
    yield* Effect.promise(() => ctx.db.insert("webhookEvents", { source, eventId, receivedAt }))
    return { fresh: true }
  })
})

/** Lets a failed handler be retried by the sender. */
export const release = effectServerMutation({
  args: { source: v.string(), eventId: v.string() },
  handler: Effect.fn("webhooks.release")(function*({ source, eventId }) {
    const seen = yield* find(source, eventId)
    const ctx = yield* MutationCtx
    if (seen) yield* Effect.promise(() => ctx.db.delete(seen._id))
  })
})

export const prune = effectInternalMutation({
  args: {},
  handler: Effect.fn("webhooks.prune")(function*() {
    const ctx = yield* MutationCtx
    const cutoff = (yield* Clock.currentTimeMillis) - 30 * DAY
    const old = yield* Effect.promise(() =>
      ctx.db.query("webhookEvents").withIndex("by_received", (q) => q.lt("receivedAt", cutoff)).take(1000)
    )
    for (const e of old) yield* Effect.promise(() => ctx.db.delete(e._id))
    return { deleted: old.length }
  })
})
