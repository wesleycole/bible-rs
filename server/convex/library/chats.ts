/**
 * Recents: saved Ask conversations. The conversation itself is a `chat` file the client uploads and
 * attaches first (so it counts toward storage); the record names it. A stale save or removal keeps
 * the server's copy and is answered `conflict`.
 */
import { Clock, Effect, Schema } from "effect"
import { InvalidRequest } from "../../shared/Errors.js"
import { Chat, type WriteAnswer } from "../../shared/records.js"
import { decideByUid } from "../lib/conflict.js"
import { effectMutation } from "../lib/effect.js"
import { Files } from "../lib/files.js"
import { Sequence } from "../lib/sequence.js"
import { CurrentUser, MutationCtx } from "../lib/services.js"
import { asRecord, Uid } from "./notes.js"

export const SaveArgs = Schema.Struct({
  uid: Uid,
  title: Chat.fields.title,
  fileUid: Uid,
  baseSeq: Schema.optionalKey(Schema.Int),
  /** When the conversation began, in server milliseconds; used only for a new one. */
  createdAt: Schema.optionalKey(Schema.Int)
})

export const RemoveArgs = Schema.Struct({ uid: Uid, baseSeq: Schema.optionalKey(Schema.Int) })

const find = Effect.fnUntraced(function*(uid: string) {
  const ctx = yield* MutationCtx
  const { userId } = yield* CurrentUser
  return yield* Effect.promise(() => ctx.db.query("chats").withIndex("by_user_uid", (q) => q.eq("userId", userId).eq("uid", uid)).unique())
})

export const save = effectMutation({
  args: SaveArgs,
  handler: Effect.fn("chats.save")(function*(args): Effect.fn.Return<WriteAnswer, InvalidRequest, MutationCtx | CurrentUser | Sequence | Files> {
    const files = yield* Files
    const file = yield* files.get(args.fileUid)
    if (!file || file.role !== "chat" || file.ownerUid !== args.uid) {
      return yield* new InvalidRequest({ message: "Attach the conversation as this chat's file first." })
    }
    const stored = yield* find(args.uid)
    const decision = decideByUid(
      stored && { seq: stored.seq, deleted: stored.deletedAt !== undefined, same: stored.fileUid === args.fileUid && stored.title === args.title },
      false,
      args.baseSeq
    )
    if (decision.kind === "unchanged") return { result: "ok", seq: decision.seq, record: yield* asRecord("chat", stored!) }
    if (decision.kind === "conflict") {
      // The losing upload is nobody's now.
      if (stored!.fileUid !== args.fileUid) yield* files.remove(args.fileUid)
      return { result: "conflict", seq: decision.seq, record: yield* asRecord("chat", stored!) }
    }
    const ctx = yield* MutationCtx
    const { userId } = yield* CurrentUser
    const seq = yield* (yield* Sequence).next
    const now = yield* Clock.currentTimeMillis
    const fields = { seq, updatedAt: now, title: args.title, fileUid: file.uid, url: file.url, bytes: file.bytes }
    let id
    if (stored) {
      const revived = stored.deletedAt !== undefined
      yield* Effect.promise(() => ctx.db.patch(stored._id, { ...fields, deletedAt: undefined, ...(revived ? { createdAt: args.createdAt ?? now } : {}) }))
      if (stored.fileUid !== file.uid) yield* files.remove(stored.fileUid)
      id = stored._id
    } else {
      id = yield* Effect.promise(() => ctx.db.insert("chats", { userId, uid: args.uid, createdAt: args.createdAt ?? now, ...fields }))
    }
    const written = (yield* Effect.promise(() => ctx.db.get(id)))!
    return { result: "ok", seq, record: yield* asRecord("chat", written) }
  })
})

export const remove = effectMutation({
  args: RemoveArgs,
  handler: Effect.fn("chats.remove")(function*(args): Effect.fn.Return<WriteAnswer, never, MutationCtx | CurrentUser | Sequence | Files> {
    const stored = yield* find(args.uid)
    const decision = decideByUid(stored && { seq: stored.seq, deleted: stored.deletedAt !== undefined, same: false }, true, args.baseSeq)
    if (decision.kind === "unchanged") {
      return { result: "ok", seq: decision.seq, ...(stored ? { record: yield* asRecord("chat", stored) } : {}) }
    }
    if (decision.kind === "conflict") return { result: "conflict", seq: decision.seq, record: yield* asRecord("chat", stored!) }
    const ctx = yield* MutationCtx
    const seq = yield* (yield* Sequence).next
    const deletedAt = yield* Clock.currentTimeMillis
    yield* Effect.promise(() => ctx.db.patch(stored!._id, { seq, deletedAt, updatedAt: deletedAt, title: "", url: "", bytes: 0 }))
    yield* (yield* Files).remove(stored!.fileUid)
    const removed = (yield* Effect.promise(() => ctx.db.get(stored!._id)))!
    return { result: "ok", seq, record: yield* asRecord("chat", removed) }
  })
})
