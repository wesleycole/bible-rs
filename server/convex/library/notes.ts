/**
 * Notes: `note` items, each with its own uid, anchored to the verses it is about. A stale write
 * never replaces a newer one: an edit made against a copy another device has since changed is kept
 * as a conflict copy beside it (`conflictOf`), and a stale removal leaves the newer note alone.
 */
import { Clock, Effect, Schema } from "effect"
import type { Doc } from "../_generated/dataModel.js"
import { InvalidRequest } from "../../shared/Errors.js"
import { Item } from "../../shared/records.js"
import { Anchors } from "../lib/anchors.js"
import { decideByUid } from "../lib/conflict.js"
import { effectMutation } from "../lib/effect.js"
import { Sequence } from "../lib/sequence.js"
import { CurrentUser, MutationCtx } from "../lib/services.js"

/** A client-made id: a UUID, or anything as short and plain. */
export const Uid = Schema.String.check(Schema.isPattern(/^[A-Za-z0-9_-]{1,64}$/))

export const UpsertArgs = Schema.Struct({
  uid: Uid,
  text: Schema.String,
  anchors: Item.fields.anchors,
  /** The `seq` the client's copy had when the edit began; absent for a first write. */
  baseSeq: Schema.optionalKey(Schema.Int),
  /** When the device made the note, in server milliseconds; used only for a new one. */
  capturedAt: Schema.optionalKey(Schema.Int)
})

export const RemoveArgs = Schema.Struct({ uid: Uid, baseSeq: Schema.optionalKey(Schema.Int) })

export type WriteResult =
  | { readonly result: "ok"; readonly seq: number }
  /**
   * The server's copy stood, and is `record`, so the client shows it at once; for a note edit, the
   * edit was kept as the note `copyUid`, written at `copySeq`, which the client's note becomes.
   */
  | {
    readonly result: "conflict"
    readonly seq: number
    readonly record: unknown
    readonly copyUid?: string
    readonly copySeq?: number
  }

/** A stored note as the feed sends it. */
export const encodeItem = Schema.encodeUnknownEffect(Item)

/** A note's title: its first line, trimmed. */
export const titleOf = (text: string) => (text.split("\n").find((l) => l.trim() !== "")?.trim() ?? "").slice(0, 80)

const MAX_TEXT = 100_000

const find = Effect.fnUntraced(function*(uid: string) {
  const ctx = yield* MutationCtx
  const { userId } = yield* CurrentUser
  return yield* Effect.promise(() => ctx.db.query("items").withIndex("by_user_uid", (q) => q.eq("userId", userId).eq("uid", uid)).unique())
})

const sameNote = (doc: Doc<"items">, text: string, anchors: ReadonlyArray<unknown>) =>
  doc.text === text && JSON.stringify(doc.anchors) === JSON.stringify(anchors)

/** Writes the note `uid` (inserting or overwriting) with the next `seq`, and its anchor index. */
const write = Effect.fnUntraced(function*(
  uid: string,
  existing: Doc<"items"> | null,
  args: { readonly text: string; readonly anchors: Item["anchors"]; readonly capturedAt?: number | undefined },
  conflictOf?: string
) {
  const ctx = yield* MutationCtx
  const { userId } = yield* CurrentUser
  const seq = yield* (yield* Sequence).next
  const now = yield* Clock.currentTimeMillis
  const fields = { seq, title: titleOf(args.text), text: args.text, anchors: [...args.anchors] }
  const id = existing
    ? yield* Effect.promise(() =>
      ctx.db.patch(existing._id, {
        ...fields,
        deletedAt: undefined,
        // A tombstone that comes back is a new note.
        ...(existing.deletedAt !== undefined ? { capturedAt: args.capturedAt ?? now } : {})
      }).then(() => existing._id)
    )
    : yield* Effect.promise(() =>
      ctx.db.insert("items", {
        userId,
        uid,
        kind: "note",
        status: "ready",
        unreviewed: false,
        files: [],
        capturedAt: args.capturedAt ?? now,
        ...(conflictOf ? { conflictOf } : {}),
        ...fields
      })
    )
  yield* (yield* Anchors).rebuild(id, args.anchors)
  return seq
})

export const upsert = effectMutation({
  args: UpsertArgs,
  handler: Effect.fn("notes.upsert")(function*(args) {
    if (args.text.length > MAX_TEXT) return yield* new InvalidRequest({ message: "That note is too long." })
    yield* (yield* Anchors).check(args.anchors)
    const stored = yield* find(args.uid)
    if (stored && stored.kind !== "note") return yield* new InvalidRequest({ message: "That record isn't a note." })
    const decision = decideByUid(
      stored && { seq: stored.seq, deleted: stored.deletedAt !== undefined, same: sameNote(stored, args.text, args.anchors) },
      false,
      args.baseSeq
    )
    switch (decision.kind) {
      case "unchanged":
        return { result: "ok", seq: decision.seq } satisfies WriteResult as WriteResult
      case "write":
        return { result: "ok", seq: yield* write(args.uid, stored, args) } satisfies WriteResult as WriteResult
      case "conflict": {
        // The edit is kept as its own note. Its uid comes from the edit's, so a retry finds it.
        const copyUid = `${args.uid}-c${args.baseSeq}`
        const copy = yield* find(copyUid)
        const copySeq = copy ? copy.seq : yield* write(copyUid, null, args, args.uid)
        const record = yield* Effect.orDie(encodeItem(stored))
        return { result: "conflict", seq: decision.seq, record, copyUid, copySeq } satisfies WriteResult as WriteResult
      }
    }
  })
})

export const remove = effectMutation({
  args: RemoveArgs,
  handler: Effect.fn("notes.remove")(function*(args) {
    const stored = yield* find(args.uid)
    const decision = decideByUid(stored && { seq: stored.seq, deleted: stored.deletedAt !== undefined, same: false }, true, args.baseSeq)
    if (decision.kind === "unchanged") return { result: "ok", seq: decision.seq } as WriteResult
    if (decision.kind === "conflict") {
      return { result: "conflict", seq: decision.seq, record: yield* Effect.orDie(encodeItem(stored)) } as WriteResult
    }
    const ctx = yield* MutationCtx
    const seq = yield* (yield* Sequence).next
    const deletedAt = yield* Clock.currentTimeMillis
    // A tombstone keeps no text: other devices only need to drop the note.
    yield* Effect.promise(() => ctx.db.patch(stored!._id, { seq, deletedAt, text: "", title: "", anchors: [] }))
    yield* (yield* Anchors).rebuild(stored!._id, [])
    return { result: "ok", seq } as WriteResult
  })
})
