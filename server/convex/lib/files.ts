/**
 * Files (docs/plans/library.md, "Files"): upload URLs, attaching what was uploaded, and deleting it,
 * with `userLibrary.storageBytes` kept current so the storage limit never scans `_storage`.
 */
import { Context, Effect, Layer } from "effect"
import type { Doc, Id } from "../_generated/dataModel.js"
import { InvalidRequest, type TooLarge } from "../../shared/Errors.js"
import { Allowance } from "./allowance.js"
import type { Item } from "../../shared/records.js"
import { Sequence } from "./sequence.js"
import { CurrentUser, MutationCtx } from "./services.js"

export type FileRole = Item["files"][number]["role"]

/** Convex keeps a file's SHA-256 as base64; clients send (and files are named by) hex. */
export const hexOf = (base64: string) =>
  Array.from(atob(base64), (c) => c.charCodeAt(0).toString(16).padStart(2, "0")).join("")

export interface Attach {
  readonly uid: string
  readonly ownerUid?: string | undefined
  readonly role: FileRole
  readonly storageId: string
  /** Hex SHA-256 of the bytes, as the client computed it; checked against what was stored. */
  readonly sha256: string
  readonly partIndex?: number | undefined
  readonly page?: number | undefined
}

export class Files extends Context.Service<Files, {
  /** A URL to POST one file's bytes to (valid an hour). */
  readonly uploadUrl: Effect.Effect<string>
  /** Records an uploaded file as `uid`, idempotently: attaching the same uid again returns the first. */
  readonly attach: (a: Attach) => Effect.Effect<Doc<"files">, InvalidRequest | TooLarge>
  /** The user's file `uid`, if there is one. */
  readonly get: (uid: string) => Effect.Effect<Doc<"files"> | null>
  /** Deletes the file's bytes and row, and gives its bytes back to the storage counter. */
  readonly remove: (uid: string) => Effect.Effect<void>
}>()("linger/convex/Files") {
  static readonly layer = Layer.effect(
    Files,
    Effect.gen(function*() {
      const ctx = yield* MutationCtx
      const { userId } = yield* CurrentUser
      const sequence = yield* Sequence
      const allowance = yield* Allowance
      const get = (uid: string) =>
        Effect.promise(() => ctx.db.query("files").withIndex("by_user_uid", (q) => q.eq("userId", userId).eq("uid", uid)).unique())
      const attach = Effect.fnUntraced(function*(a: Attach) {
        const existing = yield* get(a.uid)
        if (existing) return existing
        const storageId = ctx.db.system.normalizeId("_storage", a.storageId)
        const stored = storageId && (yield* Effect.promise(() => ctx.db.system.get(storageId)))
        if (!storageId || !stored) return yield* new InvalidRequest({ message: "No upload has that storage id." })
        const sha256 = hexOf(stored.sha256)
        if (sha256 !== a.sha256.toLowerCase()) {
          return yield* new InvalidRequest({ message: "The upload's bytes don't match its SHA-256." })
        }
        // A refused upload stays in _storage: the failed mutation would roll its deletion back.
        // It is no user's file and counts toward no limit.
        yield* allowance.checkStorage(userId, stored.size)
        const url = yield* Effect.promise(() => ctx.storage.getUrl(storageId))
        const id: Id<"files"> = yield* Effect.promise(() =>
          ctx.db.insert("files", {
            userId,
            uid: a.uid,
            ownerUid: a.ownerUid,
            role: a.role,
            storageId,
            contentType: stored.contentType ?? "application/octet-stream",
            bytes: stored.size,
            sha256,
            url: url!,
            partIndex: a.partIndex,
            page: a.page
          })
        )
        yield* sequence.addStorage(stored.size)
        return (yield* Effect.promise(() => ctx.db.get(id)))!
      })
      const remove = Effect.fnUntraced(function*(uid: string) {
        const file = yield* get(uid)
        if (!file) return
        yield* Effect.promise(async () => {
          await ctx.storage.delete(file.storageId)
          await ctx.db.delete(file._id)
        })
        yield* sequence.addStorage(-file.bytes)
      })
      return Files.of({ uploadUrl: Effect.promise(() => ctx.storage.generateUploadUrl()), attach, get, remove })
    })
  )
}
