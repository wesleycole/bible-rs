/**
 * M1 spike only: `highlights.set` written without Effect, so the test suite can compare the two.
 * Delete with the spike.
 */
import { v } from "convex/values"
import { Highlight } from "../../shared/records.js"
import { userMutation } from "../lib/user.js"
import { verseParts } from "../lib/verses.js"
import { highlightUid } from "./highlights.js"

export const setPlain = userMutation({
  args: {
    verse: v.number(),
    color: v.union(v.literal("yellow"), v.literal("green"), v.literal("blue"), v.literal("pink"), v.literal("purple"), v.literal("orange"), v.null()),
    baseSeq: v.optional(v.number())
  },
  handler: async (ctx, args) => {
    if (verseParts(args.verse) === null) throw new Error("invalid verse")
    const userId = ctx.user._id
    const existing = await ctx.db.query("highlights").withIndex("by_user_verse", (q) => q.eq("userId", userId).eq("verse", args.verse)).unique()
    if (existing && args.baseSeq !== undefined && existing.seq > args.baseSeq) {
      return { result: "conflict" as const, seq: existing.seq }
    }
    if (!existing && args.color === null) return { result: "ok" as const, seq: 0 }
    const row = await ctx.db.query("userLibrary").withIndex("by_user", (q) => q.eq("userId", userId)).unique()
    let seq: number
    if (row) {
      seq = row.seq + 1
      await ctx.db.patch(row._id, { seq })
    } else {
      seq = 1
      await ctx.db.insert("userLibrary", { userId, seq, purgedThrough: 0, storageBytes: 0 })
    }
    if (existing) {
      await ctx.db.patch(existing._id, args.color === null ? { seq, deletedAt: Date.now() } : { seq, color: args.color, deletedAt: undefined })
    } else {
      await ctx.db.insert("highlights", { userId, uid: highlightUid(args.verse), seq, verse: args.verse, color: args.color as Highlight["color"] })
    }
    return { result: "ok" as const, seq }
  }
})
