/**
 * Deleting what Linger holds (docs/plans/backend.md, "Deletion"): an account, in steps of at most
 * STEP records, ending with the `users` row; library tombstones older than 30 days, raising each
 * user's `purgedThrough` past them; and stored blobs nothing points at. Every step reads the time
 * from `Clock`. (Tests still fake `Date`: the documents' `_creationTime` the sweeps compare against
 * comes from Convex, not from `Clock`.)
 */
import { Clock, Context, Effect, Layer } from "effect"
import type { Doc, Id, TableNames } from "../_generated/dataModel.js"
import { MutationCtx } from "./services.js"
import { DAY } from "./time.js"

/** Records a step deletes before handing on to the next. */
export const STEP = 500

/** Tombstones are kept this long, so devices offline for less than it see the deletion. */
export const TOMBSTONE_DAYS = 30

/** A blob nothing points at is deleted once it is this old: uploads are attached within seconds. */
export const ORPHAN_AGE = DAY

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

const synced = ["highlights", "bookmarks", "items", "chats"] as const

export class Purge extends Context.Service<Purge, {
  /** Deletes up to STEP of a user's records; `done` once the `users` row is gone too. */
  readonly account: (userId: Id<"users">) => Effect.Effect<{ readonly done: boolean }>
  /** Hard-deletes up to STEP tombstones past TOMBSTONE_DAYS, with an item's files and index rows; `more` when the budget ran out. */
  readonly tombstones: Effect.Effect<{ readonly deleted: number; readonly more: boolean }>
  /**
   * Deletes up to STEP blobs created between `from` and ORPHAN_AGE ago that no `files` row or
   * transcript points at (an upload refused over the storage limit, or never attached). Answers
   * where the next call starts, if the budget ran out.
   */
  readonly orphans: (from: number) => Effect.Effect<{ readonly deleted: number; readonly next?: number }>
}>()("linger/convex/Purge") {
  static readonly layer = Layer.effect(
    Purge,
    Effect.gen(function*() {
      const ctx = yield* MutationCtx
      const db = <A>(f: () => Promise<A>) => Effect.promise(f)

      const remove = Effect.fnUntraced(function*(doc: Doc<any>) {
        if ("storageId" in doc && doc.storageId) yield* db(() => ctx.storage.delete(doc.storageId))
        if ("transcript" in doc && doc.transcript) yield* db(() => ctx.storage.delete(doc.transcript))
        yield* db(() => ctx.db.delete(doc._id))
      })

      const account = Effect.fnUntraced(function*(userId: Id<"users">) {
        let budget = STEP
        for (const [, read] of owned) {
          const rows = yield* db(() => read(ctx, userId, budget))
          yield* Effect.forEach(rows, remove, { discard: true })
          budget -= rows.length
          if (budget === 0) return { done: false }
        }
        if (yield* db(() => ctx.db.get(userId))) yield* db(() => ctx.db.delete(userId))
        return { done: true }
      })

      const removeItemParts = Effect.fnUntraced(function*(item: Doc<"items">) {
        const files = yield* db(() =>
          ctx.db.query("files").withIndex("by_owner", (q) => q.eq("userId", item.userId).eq("ownerUid", item.uid)).collect()
        )
        yield* Effect.forEach(files, remove, { discard: true })
        const anchors = yield* db(() => ctx.db.query("anchors").withIndex("by_item", (q) => q.eq("itemId", item._id)).collect())
        yield* Effect.forEach(anchors, (a) => db(() => ctx.db.delete(a._id)), { discard: true })
      })

      const tombstones = Effect.gen(function*() {
        const cutoff = (yield* Clock.currentTimeMillis) - TOMBSTONE_DAYS * DAY
        let budget = STEP
        const through = new Map<Id<"users">, number>()
        for (const table of synced) {
          const old = yield* db(() =>
            ctx.db.query(table).withIndex("by_deleted", (q) => q.gt("deletedAt", 0).lt("deletedAt", cutoff)).take(budget)
          )
          for (const doc of old) {
            if (table === "items") yield* removeItemParts(doc as Doc<"items">)
            yield* db(() => ctx.db.delete(doc._id))
            through.set(doc.userId, Math.max(through.get(doc.userId) ?? 0, doc.seq))
          }
          budget -= old.length
          if (budget === 0) break
        }
        for (const [userId, seq] of through) {
          const library = yield* db(() => ctx.db.query("userLibrary").withIndex("by_user", (q) => q.eq("userId", userId)).unique())
          if (library && seq > library.purgedThrough) yield* db(() => ctx.db.patch(library._id, { purgedThrough: seq }))
        }
        return { deleted: STEP - budget, more: budget === 0 }
      })

      const orphans = Effect.fnUntraced(function*(from: number) {
        const before = (yield* Clock.currentTimeMillis) - ORPHAN_AGE
        const blobs = yield* db(() =>
          ctx.db.system.query("_storage").withIndex("by_creation_time", (q) => q.gte("_creationTime", from).lt("_creationTime", before)).take(STEP)
        )
        let deleted = 0
        for (const blob of blobs) {
          const file = yield* db(() => ctx.db.query("files").withIndex("by_storage", (q) => q.eq("storageId", blob._id)).first())
          const transcript = yield* db(() => ctx.db.query("sermons").withIndex("by_transcript", (q) => q.eq("transcript", blob._id)).first())
          if (file || transcript) continue
          yield* db(() => ctx.storage.delete(blob._id))
          deleted++
        }
        return blobs.length === STEP ? { deleted, next: blobs.at(-1)!._creationTime } : { deleted }
      })

      return Purge.of({ account, tombstones, orphans })
    })
  )
}
