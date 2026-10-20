import { v } from "convex/values"
import { Clock, Effect } from "effect"
import { internal } from "./_generated/api.js"
import type { Doc, Id } from "./_generated/dataModel.js"
import { ensureUser, userByClerk } from "./lib/accounts.js"
import { Allowance } from "./lib/allowance.js"
import { effectInternalMutation, effectServerAction, effectServerMutation, effectServerQuery } from "./lib/effect.js"
import { InvalidRequest, NotFound } from "../shared/Errors.js"
import { denial, type Denial } from "./lib/errors.js"
import { ActionCtx, MutationCtx, QueryCtx } from "./lib/services.js"
import { DAY, HOUR } from "./lib/time.js"

export { TRIAL_SECONDS } from "./lib/allowance.js"

export type CreateSermon = { ok: true; sermonId: Id<"sermons">; blobPath: string; trial: boolean } | Denial

/**
 * Admits a sermon for cloud transcription: Pro with hours left, or a Free account's unused trial.
 * Reserves its duration and names the private blob it will be uploaded to.
 */
export const create = effectServerMutation({
  args: {
    clerkId: v.string(),
    recordingId: v.string(),
    durationSec: v.number(),
    bytes: v.number(),
    nonce: v.string()
  },
  handler: Effect.fn("sermons.create")(function*(args) {
    const ctx = yield* MutationCtx
    const allowance = yield* Allowance
    const now = yield* Clock.currentTimeMillis
    const user = yield* ensureUser(args.clerkId)
    return yield* Effect.gen(function*() {
      const { trial } = yield* allowance.admitSermon(user, args.durationSec)
      const blobPath = `sermons/${user._id}/${args.nonce}.wav`
      const sermon = yield* Effect.promise(async () => {
        const id = await ctx.db.insert("sermons", {
          userId: user._id,
          recordingId: args.recordingId,
          blobPath,
          durationSec: args.durationSec,
          bytes: args.bytes,
          trial,
          status: "uploading",
          createdAt: now
        })
        return (await ctx.db.get(id))!
      })
      yield* allowance.reserveSermon(sermon)
      return { ok: true as const, sermonId: sermon._id, blobPath, trial } satisfies CreateSermon as CreateSermon
    }).pipe(Effect.catch((e) => Effect.succeed<CreateSermon>(denial(e))))
  })
})

/**
 * A row of this (the proxy's) flow, with the fields it wrote. A row with an `itemId` belongs to
 * library/sermons.ts, whose workflow and crons look after it; nothing here touches those.
 */
type ProxyRow = Doc<"sermons"> & { recordingId: string; blobPath: string; bytes: number; status: "uploading" | "transcribing" | "done" | "error" }
const proxyRow = (s: Doc<"sermons"> | null): ProxyRow | null => (s && s.itemId === undefined ? (s as ProxyRow) : null)

const owned = Effect.fnUntraced(function*(clerkId: string, sermonId: string) {
  const ctx = yield* QueryCtx
  const user = yield* Effect.promise(() => userByClerk(ctx, clerkId))
  const id = ctx.db.normalizeId("sermons", sermonId)
  const sermon = proxyRow(id ? yield* Effect.promise(() => ctx.db.get(id)) : null)
  return user && sermon && sermon.userId === user._id ? sermon : null
})

const view = (s: ProxyRow) => ({
  sermonId: s._id,
  recordingId: s.recordingId,
  status: s.status,
  blobPath: s.blobPath,
  durationSec: s.durationSec,
  bytes: s.bytes,
  assemblyId: s.assemblyId,
  webhookSecretHash: s.webhookSecretHash,
  error: s.error,
  createdAt: s.createdAt
})

export const get = effectServerQuery({
  args: { clerkId: v.string(), sermonId: v.string() },
  handler: Effect.fn("sermons.get")(function*({ clerkId, sermonId }) {
    const ctx = yield* QueryCtx
    const s = yield* owned(clerkId, sermonId)
    if (!s) return null
    const transcript = s.transcript
    return { ...view(s), transcriptUrl: transcript ? yield* Effect.promise(() => ctx.storage.getUrl(transcript)) : null }
  })
})

export const startTranscription = effectServerMutation({
  args: { clerkId: v.string(), sermonId: v.string(), assemblyId: v.string(), webhookSecretHash: v.string() },
  handler: Effect.fn("sermons.startTranscription")(function*(args) {
    const ctx = yield* MutationCtx
    return yield* Effect.gen(function*() {
      const s = yield* owned(args.clerkId, args.sermonId)
      if (!s) return yield* new NotFound({ message: "No such sermon." })
      if (s.status !== "uploading") return yield* new InvalidRequest({ message: "That sermon is already being transcribed." })
      const transcribingAt = yield* Clock.currentTimeMillis
      yield* Effect.promise(() =>
        ctx.db.patch(s._id, { status: "transcribing", assemblyId: args.assemblyId, webhookSecretHash: args.webhookSecretHash, transcribingAt })
      )
      return { ok: true as const } as { ok: true } | Denial
    }).pipe(Effect.catch((e) => Effect.succeed<{ ok: true } | Denial>(denial(e))))
  })
})

export const byAssembly = effectServerQuery({
  args: { assemblyId: v.string() },
  handler: Effect.fn("sermons.byAssembly")(function*({ assemblyId }) {
    const ctx = yield* QueryCtx
    const s = proxyRow(yield* Effect.promise(() => ctx.db.query("sermons").withIndex("by_assembly", (q) => q.eq("assemblyId", assemblyId)).unique()))
    return s ? view(s) : null
  })
})

/** Keeps the finished transcript for the client to fetch (for 24 hours) and settles its audio. */
export const complete = effectServerAction({
  args: { sermonId: v.id("sermons"), transcript: v.string(), audioSeconds: v.number() },
  handler: Effect.fn("sermons.complete")(function*(args): Effect.fn.Return<{ done: boolean }, never, ActionCtx> {
    const ctx = yield* ActionCtx
    const storageId = yield* Effect.promise(() => ctx.storage.store(new Blob([args.transcript], { type: "application/json" })))
    const done: boolean = yield* Effect.promise(() =>
      ctx.runMutation(internal.sermons.markDone, { sermonId: args.sermonId, storageId, audioSeconds: args.audioSeconds })
    )
    if (!done) yield* Effect.promise(() => ctx.storage.delete(storageId))
    return { done }
  })
})

export const markDone = effectInternalMutation({
  args: { sermonId: v.id("sermons"), storageId: v.id("_storage"), audioSeconds: v.number() },
  handler: Effect.fn("sermons.markDone")(function*({ sermonId, storageId, audioSeconds }) {
    const ctx = yield* MutationCtx
    const s = yield* Effect.promise(() => ctx.db.get(sermonId))
    // A second webhook delivery, or a poll racing the webhook: the first one settled it.
    if (!s || s.status === "done") return false
    const seconds = Math.ceil(audioSeconds)
    const doneAt = yield* Clock.currentTimeMillis
    yield* Effect.promise(() => ctx.db.patch(s._id, { status: "done", transcript: storageId, audioSeconds: seconds, doneAt }))
    yield* (yield* Allowance).settleSermon(s, seconds)
    return true
  })
})

const failSermon = Effect.fnUntraced(function*(s: Doc<"sermons">, error: string) {
  const ctx = yield* MutationCtx
  yield* Effect.promise(() => ctx.db.patch(s._id, { status: "error", error }))
  yield* (yield* Allowance).releaseSermon(s)
})

export const fail = effectServerMutation({
  args: { sermonId: v.id("sermons"), error: v.string() },
  handler: Effect.fn("sermons.fail")(function*({ sermonId, error }) {
    const ctx = yield* MutationCtx
    const s = yield* Effect.promise(() => ctx.db.get(sermonId))
    if (!s || s.status === "done" || s.status === "error") return { failed: false }
    yield* failSermon(s, error)
    return { failed: true }
  })
})

/**
 * Fails sermons stuck in transcription for over 3 hours, or never uploaded within 2 hours, and
 * returns the blobs and AssemblyAI jobs to delete.
 */
export const failStuck = effectInternalMutation({
  args: {},
  handler: Effect.fn("sermons.failStuck")(function*() {
    const ctx = yield* MutationCtx
    const now = yield* Clock.currentTimeMillis
    const stuck = yield* Effect.promise(async () =>
      [
        ...(await ctx.db
          .query("sermons")
          .withIndex("by_status", (q) => q.eq("status", "transcribing").lt("createdAt", now - 3 * HOUR))
          .take(200)),
        ...(await ctx.db
          .query("sermons")
          .withIndex("by_status", (q) => q.eq("status", "uploading").lt("createdAt", now - 2 * HOUR))
          .take(200))
      ]
        .map(proxyRow)
        .filter((s): s is ProxyRow => s !== null && (s.status === "uploading" || (s.transcribingAt ?? s.createdAt) < now - 3 * HOUR))
    )
    for (const s of stuck) {
      yield* failSermon(s, s.status === "uploading" ? "The upload never finished." : "Transcription took too long.")
    }
    return {
      blobPaths: stuck.map((s) => s.blobPath),
      assemblyIds: stuck.flatMap((s) => (s.assemblyId ? [s.assemblyId] : []))
    }
  })
})

/**
 * Transcripts are kept 24 hours for the client to fetch, then deleted with their sermon rows
 * (the ledger keeps what billing needs). Failed sermons go after a week.
 */
export const expire = effectInternalMutation({
  args: {},
  handler: Effect.fn("sermons.expire")(function*() {
    const ctx = yield* MutationCtx
    const now = yield* Clock.currentTimeMillis
    const done = yield* Effect.promise(() =>
      ctx.db.query("sermons").withIndex("by_status", (q) => q.eq("status", "done").lt("createdAt", now - DAY)).take(500)
    )
    const failed = yield* Effect.promise(() =>
      ctx.db.query("sermons").withIndex("by_status", (q) => q.eq("status", "error").lt("createdAt", now - 7 * DAY)).take(500)
    )
    let deleted = 0
    for (const s of [...done.filter((s) => (s.doneAt ?? 0) < now - DAY), ...failed].filter((s) => proxyRow(s) !== null)) {
      const transcript = s.transcript
      if (transcript) yield* Effect.promise(() => ctx.storage.delete(transcript))
      yield* Effect.promise(() => ctx.db.delete(s._id))
      deleted++
    }
    return { deleted }
  })
})
