/**
 * Highlights: one per verse, keyed by the verse, last writer wins unless the writer's `baseSeq`
 * is stale, in which case the server's copy stands and the client is told.
 */
import { Effect, Schema } from "effect"
import type { InvalidRequest } from "../../shared/Errors.js"
import { Highlight } from "../../shared/records.js"
import { effectMutation, effectQuery } from "../lib/effect.js"
import { setPerVerse } from "../lib/perVerse.js"
import type { Sequence } from "../lib/sequence.js"
import { CurrentUser, MutationCtx, QueryCtx } from "../lib/services.js"
import { checkVerse } from "../lib/verses.js"

export const SetArgs = Schema.Struct({
  verse: Schema.Int,
  /** The colour, or `null` to clear the highlight. */
  color: Schema.NullOr(Highlight.fields.color),
  /** The `seq` the client's copy had when the edit began; absent for a first write. */
  baseSeq: Schema.optionalKey(Schema.Int),
  /** When the device made the record, in server milliseconds; used only for a new record. */
  createdAt: Highlight.fields.createdAt
})

export type SetResult =
  | { readonly result: "ok"; readonly seq: number }
  | { readonly result: "conflict"; readonly seq: number; readonly createdAt?: number; readonly color: Highlight["color"] | null }

/** The record's id, derived from the verse, so every device makes the same one. */
export const highlightUid = (verse: number) => `h-${verse}`

export const set = effectMutation({
  args: SetArgs,
  handler: Effect.fn("highlights.set")(function*(args): Effect.fn.Return<SetResult, InvalidRequest, MutationCtx | CurrentUser | Sequence> {
    const verse = yield* checkVerse(args.verse)
    const ctx = yield* MutationCtx
    const { userId } = yield* CurrentUser
    const result = yield* setPerVerse({
      find: Effect.promise(async () => {
        const row = await ctx.db.query("highlights").withIndex("by_user_verse", (q) => q.eq("userId", userId).eq("verse", verse)).unique()
        return row && { id: row._id, seq: row.seq, createdAt: row.createdAt, value: row.deletedAt === undefined ? row.color : null }
      }),
      insert: (seq, color: Highlight["color"], createdAt) => Effect.promise(() => ctx.db.insert("highlights", { userId, uid: highlightUid(verse), seq, createdAt, verse, color })),
      update: (id, seq, color, deletedAt, createdAt) =>
        Effect.promise(() =>
          ctx.db.patch(id, color === null ? { seq, deletedAt } : { seq, color, deletedAt: undefined, ...(createdAt === undefined ? {} : { createdAt }) })
        )
    }, args.color, args.baseSeq, args.createdAt)
    return result.result === "conflict" ? { result: "conflict", seq: result.seq, color: result.value, ...(result.createdAt === undefined ? {} : { createdAt: result.createdAt }) } : result
  })
})

/** The user's highlights, without tombstones. */
export const list = effectQuery({
  args: Schema.Struct({}),
  empty: () => Effect.succeed<Array<Highlight>>([]),
  handler: Effect.fn("highlights.list")(function*() {
    const ctx = yield* QueryCtx
    const { userId } = yield* CurrentUser
    const rows = yield* Effect.promise(() =>
      ctx.db.query("highlights").withIndex("by_user_seq", (q) => q.eq("userId", userId)).collect()
    )
    return rows.filter((r) => r.deletedAt === undefined).map((r): Highlight => ({ uid: r.uid, seq: r.seq, verse: r.verse, color: r.color }))
  })
})
