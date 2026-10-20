"use node"
/**
 * Joining a sermon's uploaded pieces into its `audio` file: a byte copy, in Node for its memory (a
 * two-hour M4A is about 44 MB). The pieces are the M4A's `uploadPart` slices when the device
 * finished, or the ADTS `audioPart`s a dead device left, which concatenate to an `audio/aac` stream
 * players estimate the timeline of.
 */
import { v } from "convex/values"
import { Effect } from "effect"
import { internal } from "../_generated/api.js"
import { InvalidRequest } from "../../shared/Errors.js"
import { effectInternalAction } from "../lib/effect.js"
import { sha256Hex } from "../lib/files.js"
import { ActionCtx } from "../lib/services.js"

/** The sermon's `audio` file (its URL), joined from its pieces when it has only those; null when nothing was uploaded. */
export const join = effectInternalAction({
  args: { sermonId: v.id("sermons") },
  handler: Effect.fn("audio.join")(function*({ sermonId }): Effect.fn.Return<{ url: string } | null, InvalidRequest, ActionCtx> {
    const ctx = yield* ActionCtx
    const pieces = yield* Effect.promise(() => ctx.runQuery(internal.library.sermons.pieces, { sermonId }))
    if (pieces.audio) return pieces.audio
    if (pieces.slices.length === 0) return null
    const blobs = yield* Effect.promise(() => Promise.all(pieces.slices.map((s) => ctx.storage.get(s.storageId))))
    if (blobs.some((b) => b === null)) return yield* new InvalidRequest({ message: "A slice of the upload is missing." })
    const buffers = yield* Effect.promise(() => Promise.all(blobs.map((b) => b!.arrayBuffer())))
    const bytes = new Uint8Array(buffers.reduce((n, b) => n + b.byteLength, 0))
    let at = 0
    for (const b of buffers) {
      bytes.set(new Uint8Array(b), at)
      at += b.byteLength
    }
    const sha256 = yield* Effect.promise(() => sha256Hex(bytes))
    if (pieces.expected !== undefined && sha256 !== pieces.expected) {
      return yield* new InvalidRequest({ message: "The joined upload doesn't match the SHA-256 the device sent." })
    }
    const storageId = yield* Effect.promise(() => ctx.storage.store(new Blob([bytes], { type: pieces.contentType })))
    return yield* Effect.promise(() => ctx.runMutation(internal.library.sermons.joined, { sermonId, storageId, sha256 }))
  })
})
