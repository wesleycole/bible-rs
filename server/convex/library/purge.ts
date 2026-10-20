/**
 * Deleting what Linger holds (docs/plans/backend.md, "Deletion"). A library can outgrow one
 * mutation's write limit, so an account is purged by a chain of scheduled steps, each deleting at
 * most STEP records (with their files) and scheduling the next, ending with the `users` row. A
 * daily cron hard-deletes library tombstones older than 30 days the same way, and raises each
 * user's `purgedThrough` past them, so a device that never saw a deletion is told to resync.
 */
import { v } from "convex/values"
import { Effect } from "effect"
import { internal } from "../_generated/api.js"
import type { Doc, Id, TableNames } from "../_generated/dataModel.js"
import { effectInternalMutation } from "../lib/effect.js"
import { MutationCtx } from "../lib/services.js"
import { DAY } from "../lib/time.js"

/** Records a step deletes before handing on to the next. */
export const STEP = 500

/** Tombstones are kept this long, so devices offline for less than it see the deletion. */
export const TOMBSTONE_DAYS = 30

type Ctx = MutationCtx["Service"]

/** Every table holding a user's rows, with how to read up to `n` of them; `users` goes last, on its own. */
const owned: ReadonlyArray<readonly [TableNames, (ctx: Ctx, userId: Id<"users">, n: number) => Promise<Array<Doc<any>>>]> = [
  ["files", (ctx, u, n) => ctx.db.query("files").withIndex("by_user_uid", (q) => q.eq("userId", u)).take(n)],
  ["anchors", (ctx, u, n) => ctx.db.query("anchors").withIndex("by_user_chapter", (q) => q.eq("userId", u)).take(n)],
  ["items", (ctx, u, n) => ctx.db.query("items").withIndex("by_user_seq", (q) => q.eq("userId", u)).take(n)],
  ["chats", (ctx, u, n) => ctx.db.query("chats").withIndex("by_user_seq", (q) => q.eq("userId", u)).take(n)],
  ["highlights", (ctx, u, n) => ctx.db.query("highlights").withIndex("by_user_seq", (q) => q.eq("userId", u)).take(n)],
  ["bookmarks", (ctx, u, n) => ctx.db.query("bookmarks").withIndex("by_user_seq", (q) => q.eq("userId", u)).take(n)],
  ["userLibrary", (ctx, u, n) => ctx.db.query("userLibrary").withIndex("by_user", (q) => q.eq("userId", u)).take(n)],
  ["sermons", (ctx, u, n) => ctx.db.query("sermons").withIndex("by_user", (q) => q.eq("userId", u)).take(n)],
  ["entitlements", (ctx, u, n) => ctx.db.query("entitlements").withIndex("by_user", (q) => q.eq("userId", u)).take(n)],
  ["usage", (ctx, u, n) => ctx.db.query("usage").withIndex("by_user_period", (q) => q.eq("userId", u)).take(n)],
  ["reservations", (ctx, u, n) => ctx.db.query("reservations").withIndex("by_user", (q) => q.eq("userId", u)).take(n)],
  ["ledger", (ctx, u, n) => ctx.db.query("ledger").withIndex("by_user_at", (q) => q.eq("userId", u)).take(n)],
  // Kept a week and with no user index: a filtered scan of a small table.
  ["questions", (ctx, u, n) => ctx.db.query("questions").filter((q) => q.eq(q.field("userId"), u)).take(n)]
]

/** Deletes a row and the stored bytes it holds. */
const remove = async (ctx: Ctx, doc: Doc<any>) => {
  if ("storageId" in doc && doc.storageId) await ctx.storage.delete(doc.storageId)
  if ("transcript" in doc && doc.transcript) await ctx.storage.delete(doc.transcript)
  await ctx.db.delete(doc._id)
}

/**
 * Deletes up to STEP of a user's records, ending with the `users` row; schedules the next step
 * when more remain. The first step runs inside the request that starts the purge, so a small
 * account is gone when it returns.
 */
export const purgeSome = async (ctx: Ctx, userId: Id<"users">): Promise<{ done: boolean }> => {
  let budget = STEP
  for (const [, read] of owned) {
    const rows = await read(ctx, userId, budget)
    for (const r of rows) await remove(ctx, r)
    budget -= rows.length
    if (budget === 0) {
      await ctx.scheduler.runAfter(0, internal.library.purge.step, { userId })
      return { done: false }
    }
  }
  if (await ctx.db.get(userId)) await ctx.db.delete(userId)
  return { done: true }
}

/** One link of an account's purge chain. */
export const step = effectInternalMutation({
  args: { userId: v.id("users") },
  handler: Effect.fn("purge.step")(function*({ userId }) {
    const ctx = yield* MutationCtx
    return yield* Effect.promise(() => purgeSome(ctx, userId))
  })
})

const synced = ["highlights", "bookmarks", "items", "chats"] as const

/** The daily cron's link: hard-deletes up to STEP tombstones past their 30 days, oldest first. */
export const tombstones = effectInternalMutation({
  args: {},
  handler: Effect.fn("purge.tombstones")(function*() {
    const ctx = yield* MutationCtx
    const cutoff = Date.now() - TOMBSTONE_DAYS * DAY
    return yield* Effect.promise(async () => {
      let budget = STEP
      const through = new Map<Id<"users">, number>()
      for (const table of synced) {
        const old = await ctx.db
          .query(table)
          .withIndex("by_deleted", (q) => q.gt("deletedAt", 0).lt("deletedAt", cutoff))
          .take(budget)
        for (const doc of old) {
          // An item's own files and index rows go with it.
          if (table === "items") {
            const files = await ctx.db.query("files").withIndex("by_owner", (q) => q.eq("userId", doc.userId).eq("ownerUid", doc.uid)).collect()
            for (const f of files) await remove(ctx, f)
            const anchors = await ctx.db.query("anchors").withIndex("by_item", (q) => q.eq("itemId", doc._id as Id<"items">)).collect()
            for (const a of anchors) await ctx.db.delete(a._id)
          }
          await ctx.db.delete(doc._id)
          through.set(doc.userId, Math.max(through.get(doc.userId) ?? 0, doc.seq))
        }
        budget -= old.length
        if (budget === 0) break
      }
      for (const [userId, seq] of through) {
        const library = await ctx.db.query("userLibrary").withIndex("by_user", (q) => q.eq("userId", userId)).unique()
        if (library && seq > library.purgedThrough) await ctx.db.patch(library._id, { purgedThrough: seq })
      }
      if (budget === 0) await ctx.scheduler.runAfter(0, internal.library.purge.tombstones, {})
      return { deleted: STEP - budget }
    })
  })
})
