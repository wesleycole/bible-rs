/**
 * Bookmarks: one per verse with its label, keyed by the verse, last writer wins unless the
 * writer's `baseSeq` is stale, in which case the server's copy stands and the client is told.
 */
import { Effect, Schema } from "effect"
import type { InvalidRequest } from "../../shared/Errors.js"
import { Bookmark } from "../../shared/records.js"
import { effectMutation } from "../lib/effect.js"
import { setPerVerse } from "../lib/perVerse.js"
import type { Sequence } from "../lib/sequence.js"
import { CurrentUser, MutationCtx } from "../lib/services.js"
import { checkVerse } from "../lib/verses.js"

export const SetArgs = Schema.Struct({
  verse: Schema.Int,
  /** The label (empty for none), or `null` to remove the bookmark. */
  label: Schema.NullOr(Bookmark.fields.label),
  /** The `seq` the client's copy had when the edit began; absent for a first write. */
  baseSeq: Schema.optionalKey(Schema.Int),
  /** When the device made the record, in server milliseconds; used only for a new record. */
  createdAt: Bookmark.fields.createdAt
})

export type SetResult =
  | { readonly result: "ok"; readonly seq: number }
  | { readonly result: "conflict"; readonly seq: number; readonly createdAt?: number; readonly label: string | null }

/** The record's id, derived from the verse, so every device makes the same one. */
export const bookmarkUid = (verse: number) => `b-${verse}`

export const set = effectMutation({
  args: SetArgs,
  handler: Effect.fn("bookmarks.set")(function*(args): Effect.fn.Return<SetResult, InvalidRequest, MutationCtx | CurrentUser | Sequence> {
    const verse = yield* checkVerse(args.verse)
    const ctx = yield* MutationCtx
    const { userId } = yield* CurrentUser
    const result = yield* setPerVerse({
      find: Effect.promise(async () => {
        const row = await ctx.db.query("bookmarks").withIndex("by_user_verse", (q) => q.eq("userId", userId).eq("verse", verse)).unique()
        return row && { id: row._id, seq: row.seq, createdAt: row.createdAt, value: row.deletedAt === undefined ? row.label : null }
      }),
      insert: (seq, label: string, createdAt) => Effect.promise(() => ctx.db.insert("bookmarks", { userId, uid: bookmarkUid(verse), seq, createdAt, verse, label })),
      update: (id, seq, label, deletedAt, createdAt) =>
        Effect.promise(() =>
          ctx.db.patch(id, label === null ? { seq, deletedAt } : { seq, label, deletedAt: undefined, ...(createdAt === undefined ? {} : { createdAt }) })
        )
    }, args.label, args.baseSeq, args.createdAt)
    return result.result === "conflict" ? { result: "conflict", seq: result.seq, label: result.value, ...(result.createdAt === undefined ? {} : { createdAt: result.createdAt }) } : result
  })
})
