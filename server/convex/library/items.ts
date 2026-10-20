/**
 * An item's details: a sermon's title, speaker, church and times, and which sermon a slide or page
 * belongs to and where in it (docs/plans/library.md, "Sermons"). A note's text and anchors are its
 * own write (`notes.upsert`); this never touches them. A stale `baseSeq` keeps the server's and
 * answers `conflict`, as for a chat.
 */
import { Clock, Effect, Schema } from "effect"
import type { Doc } from "../_generated/dataModel.js"
import { InvalidRequest } from "../../shared/Errors.js"
import { Item, type WriteAnswer } from "../../shared/records.js"
import { decideByUid } from "../lib/conflict.js"
import { effectMutation } from "../lib/effect.js"
import { Sequence } from "../lib/sequence.js"
import { CurrentUser, MutationCtx } from "../lib/services.js"
import { asRecord, Uid } from "./notes.js"

export const UpdateDetailsArgs = Schema.Struct({
  uid: Uid,
  title: Schema.optionalKey(Item.fields.title),
  /** The recording's details, or `null` to clear them. */
  details: Schema.optionalKey(Schema.NullOr(Item.fields.details.schema)),
  /** The sermon this item was captured during, or `null` to detach it. */
  sermonUid: Schema.optionalKey(Schema.NullOr(Uid)),
  /** Where in that sermon, in milliseconds, or `null` for unknown. */
  offsetMs: Schema.optionalKey(Schema.NullOr(Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)))),
  baseSeq: Schema.optionalKey(Schema.Int)
})

const MAX_TITLE = 200

const find = Effect.fnUntraced(function*(uid: string) {
  const ctx = yield* MutationCtx
  const { userId } = yield* CurrentUser
  return yield* Effect.promise(() => ctx.db.query("items").withIndex("by_user_uid", (q) => q.eq("userId", userId).eq("uid", uid)).unique())
})

export const updateDetails = effectMutation({
  args: UpdateDetailsArgs,
  handler: Effect.fn("items.updateDetails")(function*(args): Effect.fn.Return<WriteAnswer, InvalidRequest, MutationCtx | CurrentUser | Sequence> {
    const stored = yield* find(args.uid)
    if (!stored || stored.deletedAt !== undefined) return yield* new InvalidRequest({ message: "That item isn't in your library." })
    if (args.title !== undefined && args.title.length > MAX_TITLE) return yield* new InvalidRequest({ message: "That title is too long." })
    if (args.sermonUid) {
      const sermon = yield* find(args.sermonUid)
      if (!sermon || sermon.deletedAt !== undefined || sermon.kind !== "sermon" || sermon.uid === stored.uid) {
        return yield* new InvalidRequest({ message: "That sermon isn't in your library." })
      }
    }
    const patch: Partial<Doc<"items">> = {
      ...(args.title !== undefined ? { title: args.title } : {}),
      ...(args.details !== undefined ? { details: args.details ?? undefined } : {}),
      ...(args.sermonUid !== undefined ? { sermonUid: args.sermonUid ?? undefined } : {}),
      ...(args.offsetMs !== undefined ? { offsetMs: args.offsetMs ?? undefined } : {})
    }
    const same = Object.entries(patch).every(([k, v]) => JSON.stringify(stored[k as keyof Doc<"items">]) === JSON.stringify(v))
    const decision = decideByUid({ seq: stored.seq, deleted: false, same }, false, args.baseSeq)
    if (decision.kind === "unchanged") return { result: "ok", seq: decision.seq, record: yield* asRecord("item", stored) }
    if (decision.kind === "conflict") return { result: "conflict", seq: decision.seq, record: yield* asRecord("item", stored) }
    const ctx = yield* MutationCtx
    const seq = yield* (yield* Sequence).next
    const updatedAt = yield* Clock.currentTimeMillis
    yield* Effect.promise(() => ctx.db.patch(stored._id, { ...patch, seq, updatedAt }))
    const written = (yield* Effect.promise(() => ctx.db.get(stored._id)))!
    return { result: "ok", seq, record: yield* asRecord("item", written) }
  })
})
