import { v } from "convex/values"
import { internalMutation } from "./_generated/server.js"
import { serverMutation } from "./lib/server.js"
import { DAY } from "./lib/time.js"

/** Records a webhook delivery; `fresh` is false when this event was already handled. */
export const claim = serverMutation({
  args: { source: v.string(), eventId: v.string() },
  handler: async (ctx, { source, eventId }) => {
    const seen = await ctx.db
      .query("webhookEvents")
      .withIndex("by_source_event", (q) => q.eq("source", source).eq("eventId", eventId))
      .unique()
    if (seen) return { fresh: false }
    await ctx.db.insert("webhookEvents", { source, eventId, receivedAt: Date.now() })
    return { fresh: true }
  }
})

/** Lets a failed handler be retried by the sender. */
export const release = serverMutation({
  args: { source: v.string(), eventId: v.string() },
  handler: async (ctx, { source, eventId }) => {
    const seen = await ctx.db
      .query("webhookEvents")
      .withIndex("by_source_event", (q) => q.eq("source", source).eq("eventId", eventId))
      .unique()
    if (seen) await ctx.db.delete(seen._id)
  }
})

export const prune = internalMutation({
  args: {},
  handler: async (ctx) => {
    const old = await ctx.db
      .query("webhookEvents")
      .withIndex("by_received", (q) => q.lt("receivedAt", Date.now() - 30 * DAY))
      .take(1000)
    for (const e of old) await ctx.db.delete(e._id)
    return { deleted: old.length }
  }
})
