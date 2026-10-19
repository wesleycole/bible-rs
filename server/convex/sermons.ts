import { v } from "convex/values"
import { internal } from "./_generated/api.js"
import type { Doc, Id } from "./_generated/dataModel.js"
import { internalMutation, type MutationCtx } from "./_generated/server.js"
import { effectivePlan, ensureUsage, ensureUser, loadPlan, outstanding, userByClerk } from "./lib/accounts.js"
import { deny, type Denial } from "./lib/errors.js"
import { transcriptionCost } from "./lib/pricing.js"
import { serverAction, serverMutation, serverQuery } from "./lib/server.js"
import { DAY, HOUR } from "./lib/time.js"

/** The Free plan's one-time cloud trial: one sermon of up to an hour. */
export const TRIAL_SECONDS = 3600

export type CreateSermon = { ok: true; sermonId: Id<"sermons">; blobPath: string; trial: boolean } | Denial

/**
 * Admits a sermon for cloud transcription: Pro with hours left, or a Free account's unused trial.
 * Reserves its duration and names the private blob it will be uploaded to.
 */
export const create = serverMutation({
  args: {
    clerkId: v.string(),
    recordingId: v.string(),
    durationSec: v.number(),
    bytes: v.number(),
    nonce: v.string()
  },
  handler: async (ctx, args): Promise<CreateSermon> => {
    const now = Date.now()
    const user = await ensureUser(ctx, args.clerkId)
    const { plan: planKey } = await effectivePlan(ctx, user._id, now)
    const plan = await loadPlan(ctx, planKey)
    let trial = false
    if (planKey === "pro" && plan.sermonSecondsPerMonth > 0) {
      const usage = await ensureUsage(ctx, user._id, now)
      const held = await outstanding(ctx, user._id)
      if (usage.sermonSeconds + held.seconds + args.durationSec > plan.sermonSecondsPerMonth) {
        return deny("allowance_exhausted", "This sermon is longer than the transcription time left this month.")
      }
    } else {
      if (user.trialHourUsed) {
        return deny("plan_required", "Cloud transcripts are part of Linger Pro.", { upgrade: true })
      }
      if (args.durationSec > TRIAL_SECONDS) {
        return deny("plan_required", "The free cloud trial covers one sermon of up to an hour.", { upgrade: true })
      }
      trial = true
      await ctx.db.patch(user._id, { trialHourUsed: true })
    }
    const blobPath = `sermons/${user._id}/${args.nonce}.wav`
    const sermonId = await ctx.db.insert("sermons", {
      userId: user._id,
      recordingId: args.recordingId,
      blobPath,
      durationSec: args.durationSec,
      bytes: args.bytes,
      trial,
      status: "uploading",
      createdAt: now
    })
    await ctx.db.insert("reservations", {
      userId: user._id,
      kind: "sermon",
      seconds: args.durationSec,
      sermonId,
      createdAt: now
    })
    return { ok: true, sermonId, blobPath, trial }
  }
})

const owned = async (ctx: Parameters<typeof userByClerk>[0], clerkId: string, sermonId: string) => {
  const user = await userByClerk(ctx, clerkId)
  const id = ctx.db.normalizeId("sermons", sermonId)
  const sermon = id ? await ctx.db.get(id) : null
  return user && sermon && sermon.userId === user._id ? sermon : null
}

const view = (s: Doc<"sermons">) => ({
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

export const get = serverQuery({
  args: { clerkId: v.string(), sermonId: v.string() },
  handler: async (ctx, { clerkId, sermonId }) => {
    const s = await owned(ctx, clerkId, sermonId)
    if (!s) return null
    return { ...view(s), transcriptUrl: s.transcript ? await ctx.storage.getUrl(s.transcript) : null }
  }
})

export const startTranscription = serverMutation({
  args: { clerkId: v.string(), sermonId: v.string(), assemblyId: v.string(), webhookSecretHash: v.string() },
  handler: async (ctx, args) => {
    const s = await owned(ctx, args.clerkId, args.sermonId)
    if (!s) return deny("not_found", "No such sermon.")
    if (s.status !== "uploading") return deny("invalid_request", "That sermon is already being transcribed.")
    await ctx.db.patch(s._id, {
      status: "transcribing",
      assemblyId: args.assemblyId,
      webhookSecretHash: args.webhookSecretHash,
      transcribingAt: Date.now()
    })
    return { ok: true as const }
  }
})

export const byAssembly = serverQuery({
  args: { assemblyId: v.string() },
  handler: async (ctx, { assemblyId }) => {
    const s = await ctx.db.query("sermons").withIndex("by_assembly", (q) => q.eq("assemblyId", assemblyId)).unique()
    return s ? view(s) : null
  }
})

/** Keeps the finished transcript for the client to fetch (for 24 hours) and settles its audio. */
export const complete = serverAction({
  args: { sermonId: v.id("sermons"), transcript: v.string(), audioSeconds: v.number() },
  handler: async (ctx, args): Promise<{ done: boolean }> => {
    const storageId = await ctx.storage.store(new Blob([args.transcript], { type: "application/json" }))
    const done: boolean = await ctx.runMutation(internal.sermons.markDone, {
      sermonId: args.sermonId,
      storageId,
      audioSeconds: args.audioSeconds
    })
    if (!done) await ctx.storage.delete(storageId)
    return { done }
  }
})

const reservationFor = (ctx: MutationCtx, s: Doc<"sermons">) =>
  ctx.db
    .query("reservations")
    .withIndex("by_user", (q) => q.eq("userId", s.userId))
    .filter((q) => q.eq(q.field("sermonId"), s._id))
    .first()

export const markDone = internalMutation({
  args: { sermonId: v.id("sermons"), storageId: v.id("_storage"), audioSeconds: v.number() },
  handler: async (ctx, { sermonId, storageId, audioSeconds }) => {
    const s = await ctx.db.get(sermonId)
    // A second webhook delivery, or a poll racing the webhook: the first one settled it.
    if (!s || s.status === "done") return false
    const now = Date.now()
    const seconds = Math.ceil(audioSeconds)
    await ctx.db.patch(s._id, { status: "done", transcript: storageId, audioSeconds: seconds, doneAt: now })
    const reservation = await reservationFor(ctx, s)
    if (reservation) await ctx.db.delete(reservation._id)
    if (!s.trial) {
      const usage = await ensureUsage(ctx, s.userId, now)
      await ctx.db.patch(usage._id, { sermonSeconds: usage.sermonSeconds + seconds })
    }
    await ctx.db.insert("ledger", {
      userId: s.userId,
      kind: "sermon",
      audioSeconds: seconds,
      costMicros: transcriptionCost(seconds),
      durationMs: now - (s.transcribingAt ?? s.createdAt),
      status: s.trial ? "trial" : "ok",
      at: now
    })
    return true
  }
})

const failSermon = async (ctx: MutationCtx, s: Doc<"sermons">, error: string) => {
  await ctx.db.patch(s._id, { status: "error", error })
  const reservation = await reservationFor(ctx, s)
  if (reservation) await ctx.db.delete(reservation._id)
  // A trial that failed through no fault of the user's is given back.
  if (s.trial) await ctx.db.patch(s.userId, { trialHourUsed: false })
}

export const fail = serverMutation({
  args: { sermonId: v.id("sermons"), error: v.string() },
  handler: async (ctx, { sermonId, error }) => {
    const s = await ctx.db.get(sermonId)
    if (!s || s.status === "done" || s.status === "error") return { failed: false }
    await failSermon(ctx, s, error)
    return { failed: true }
  }
})

/**
 * Fails sermons stuck in transcription for over 3 hours, or never uploaded within 2 hours, and
 * returns the blobs and AssemblyAI jobs to delete.
 */
export const failStuck = internalMutation({
  args: {},
  handler: async (ctx) => {
    const now = Date.now()
    const stuck = [
      ...(await ctx.db
        .query("sermons")
        .withIndex("by_status", (q) => q.eq("status", "transcribing").lt("createdAt", now - 3 * HOUR))
        .take(200)),
      ...(await ctx.db
        .query("sermons")
        .withIndex("by_status", (q) => q.eq("status", "uploading").lt("createdAt", now - 2 * HOUR))
        .take(200))
    ].filter((s) => s.status === "uploading" || (s.transcribingAt ?? s.createdAt) < now - 3 * HOUR)
    for (const s of stuck) {
      await failSermon(ctx, s, s.status === "uploading" ? "The upload never finished." : "Transcription took too long.")
    }
    return {
      blobPaths: stuck.map((s) => s.blobPath),
      assemblyIds: stuck.flatMap((s) => (s.assemblyId ? [s.assemblyId] : []))
    }
  }
})

/**
 * Transcripts are kept 24 hours for the client to fetch, then deleted with their sermon rows
 * (the ledger keeps what billing needs). Failed sermons go after a week.
 */
export const expire = internalMutation({
  args: {},
  handler: async (ctx) => {
    const now = Date.now()
    const done = await ctx.db
      .query("sermons")
      .withIndex("by_status", (q) => q.eq("status", "done").lt("createdAt", now - DAY))
      .take(500)
    const failed = await ctx.db
      .query("sermons")
      .withIndex("by_status", (q) => q.eq("status", "error").lt("createdAt", now - 7 * DAY))
      .take(500)
    let deleted = 0
    for (const s of [...done.filter((s) => (s.doneAt ?? 0) < now - DAY), ...failed]) {
      if (s.transcript) await ctx.storage.delete(s.transcript)
      await ctx.db.delete(s._id)
      deleted++
    }
    return { deleted }
  }
})
