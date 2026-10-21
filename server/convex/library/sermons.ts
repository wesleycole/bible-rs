/**
 * Sermons (docs/plans/library.md, "Sermons"). A sermon is two rows: the `items` row the user sees,
 * and a `sermons` row, its transcription job, that billing and the webhook see. `begin` makes both
 * as `recording`; the device attaches ADTS parts as it records and the finished M4A in `uploadPart`
 * slices; `finish` starts the workflow below, which joins the slices into the `audio` file and,
 * when the plan allows, has the Transcriber turn it into the item's `text` file. A sermon that
 * can't be transcribed in the cloud `waiting`s for the device (or "Transcribe now").
 */
import { cancel, sendEvent, start, vResultValidator, vWorkflowId, type WorkflowId, WorkflowManager } from "@convex-dev/workflow"
import { v } from "convex/values"
import { Clock, Effect, Option, Schema } from "effect"
import { components, internal } from "../_generated/api.js"
import type { Doc, Id } from "../_generated/dataModel.js"
import { AllowanceExhausted, InvalidRequest, PlanRequired, UpstreamFailed, type UpstreamUnavailable } from "../../shared/Errors.js"
import { Item, type Transcript, TranscriptFields, type WriteAnswer } from "../../shared/records.js"
import { Allowance } from "../lib/allowance.js"
import { effectInternalAction, effectInternalMutation, effectInternalQuery, effectMutation, userServices } from "../lib/effect.js"
import { Files, sha256Hex } from "../lib/files.js"
import { Sequence } from "../lib/sequence.js"
import { ActionCtx, CurrentUser, MutationCtx, QueryCtx } from "../lib/services.js"
import { HOUR, MINUTE } from "../lib/time.js"
import { Transcriber } from "../lib/transcriber.js"
import { asRecord, Uid } from "./notes.js"

/** A webhook is late once a job has run this long; the hourly cron then asks the provider itself. */
const LATE = 30 * MINUTE
/** A job still running after this long has failed. */
const STUCK = 3 * HOUR
/** A recording with no new part for this long, and no `finish`, is over: its device died. */
const DEAD = 2 * HOUR

const EXCERPT = 200

const db = <A>(f: () => Promise<A>) => Effect.promise(f)

// ---------------------------------------------------------------------------------------------
// Writing the two rows

/** The item's `files` as the record carries them: every file of the owner but the slices, in part order. */
const filesOf = Effect.fnUntraced(function*(sermon: Doc<"sermons">, item: Doc<"items">) {
  const ctx = yield* MutationCtx
  const rows = yield* db(() => ctx.db.query("files").withIndex("by_owner", (q) => q.eq("userId", sermon.userId).eq("ownerUid", item.uid)).collect())
  const seconds = sermon.audioSeconds ?? sermon.durationSec
  return rows
    .filter((f) => f.role !== "uploadPart")
    .sort((a, b) => (a.partIndex ?? 0) - (b.partIndex ?? 0))
    .map((f) => ({
      uid: f.uid,
      role: f.role,
      contentType: f.contentType,
      bytes: f.bytes,
      url: f.url,
      ...(f.page !== undefined ? { page: f.page } : {}),
      ...(f.role === "audio" && seconds > 0 ? { durationMs: seconds * 1000 } : {})
    }))
})

/** Patches the sermon's item with the next `seq` and its current files, and answers it as stored. */
const writeItem = Effect.fnUntraced(function*(
  sermon: Doc<"sermons">,
  patch: Partial<Doc<"items">>
): Effect.fn.Return<Doc<"items">, never, MutationCtx | Sequence> {
  const ctx = yield* MutationCtx
  const item = (yield* db(() => ctx.db.get(sermon.itemId!)))!
  const files = yield* filesOf(sermon, item)
  const seq = yield* (yield* Sequence).next
  const updatedAt = yield* Clock.currentTimeMillis
  yield* db(() => ctx.db.patch(item._id, { ...patch, files, seq, updatedAt }))
  return (yield* db(() => ctx.db.get(item._id)))!
})

const userOf = Effect.fnUntraced(function*(sermon: Doc<"sermons">) {
  const ctx = yield* MutationCtx
  return (yield* db(() => ctx.db.get(sermon.userId)))!
})

/** A step's write to a sermon's item, with the library services for its owner. */
const asOwner = <A, E>(sermon: Doc<"sermons">, effect: Effect.Effect<A, E, MutationCtx | Sequence | Files>) =>
  Effect.flatMap(userOf(sermon), (user) => effect.pipe(Effect.provide(userServices(user))))

/** Starts the workflow for a sermon whose audio has been (or is being) uploaded, and records it on both rows. */
const startWorkflow = Effect.fnUntraced(function*(sermon: Doc<"sermons">, transcribe: boolean) {
  const ctx = yield* MutationCtx
  const workflowId = yield* db(() =>
    start(ctx, internal.library.sermons.transcription, { sermonId: sermon._id, transcribe }, {
      onComplete: internal.library.sermons.onComplete,
      context: sermon._id
    })
  )
  yield* db(() => ctx.db.patch(sermon._id, { status: "uploading", workflowId }))
  return yield* writeItem(sermon, { status: "uploading" })
})

/** Ends a sermon in error and gives back its reservation (and a trial). */
const failSermon = Effect.fnUntraced(function*(sermon: Doc<"sermons">, error: string) {
  if (sermon.status === "done" || sermon.status === "error") return
  const ctx = yield* MutationCtx
  yield* db(() => ctx.db.patch(sermon._id, { status: "error", error }))
  yield* (yield* Allowance).releaseSermon(sermon)
  yield* asOwner(sermon, writeItem(sermon, { status: "error" }))
})

// ---------------------------------------------------------------------------------------------
// The apps' functions

const find = Effect.fnUntraced(function*(uid: string) {
  const ctx = yield* MutationCtx
  const { userId } = yield* CurrentUser
  const item = yield* db(() => ctx.db.query("items").withIndex("by_user_uid", (q) => q.eq("userId", userId).eq("uid", uid)).unique())
  if (!item || item.deletedAt !== undefined || item.kind !== "sermon") return yield* new InvalidRequest({ message: "That sermon isn't in your library." })
  const sermon = yield* db(() => ctx.db.query("sermons").withIndex("by_item", (q) => q.eq("itemId", item._id)).unique())
  if (!sermon) return yield* new InvalidRequest({ message: "That sermon wasn't recorded through Linger." })
  return { item, sermon }
})

/** What this recording may use, from the plan: Pro uploads audio always and transcribes with hours left; Free only for its unused trial. */
const allowed = Effect.fnUntraced(function*() {
  const ctx = yield* MutationCtx
  const { userId } = yield* CurrentUser
  const user = (yield* db(() => ctx.db.get(userId)))!
  const summary = yield* (yield* Allowance).summary(user)
  return summary.plan === "pro"
    ? { uploadAudio: true, cloudTranscript: summary.sermons.hoursLeft > 0 }
    : { uploadAudio: summary.sermons.trialAvailable, cloudTranscript: summary.sermons.trialAvailable }
})

export const BeginArgs = Schema.Struct({
  uid: Uid,
  details: Item.fields.details.schema,
  recordedOn: Item.fields.recordedOn.schema
})

/** Starts a recording: the item and its job, both `recording`. Beginning the same uid again only answers again. */
export const begin = effectMutation({
  args: BeginArgs,
  handler: Effect.fn("sermons.begin")(function*(args) {
    const ctx = yield* MutationCtx
    const { userId } = yield* CurrentUser
    const existing = yield* db(() => ctx.db.query("items").withIndex("by_user_uid", (q) => q.eq("userId", userId).eq("uid", args.uid)).unique())
    if (existing && existing.kind !== "sermon") return yield* new InvalidRequest({ message: "That record isn't a sermon." })
    if (!existing) {
      const seq = yield* (yield* Sequence).next
      const now = yield* Clock.currentTimeMillis
      const itemId = yield* db(() =>
        ctx.db.insert("items", {
          userId,
          uid: args.uid,
          kind: "sermon",
          status: "recording",
          title: args.details.title ?? "",
          details: args.details,
          recordedOn: args.recordedOn,
          unreviewed: false,
          files: [],
          anchors: [],
          capturedAt: now,
          seq,
          updatedAt: now
        })
      )
      yield* db(() => ctx.db.insert("sermons", { userId, itemId, durationSec: 0, trial: false, status: "recording", createdAt: now }))
    }
    return yield* allowed()
  })
})

export const FinishArgs = Schema.Struct({
  uid: Uid,
  durationMs: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
  /** The uid of the uploaded M4A (the hex SHA-256 of its bytes), whose `uploadPart` slices are attached; absent when no audio was uploaded. */
  audioFileUid: Schema.optionalKey(Uid)
})

/**
 * The recording has ended and everything it uploads is attached. With audio, the workflow joins
 * the slices and, if the plan admits the duration now, transcribes; otherwise the sermon waits.
 */
export const finish = effectMutation({
  args: FinishArgs,
  handler: Effect.fn("sermons.finish")(function*(args): Effect.fn.Return<WriteAnswer, InvalidRequest, MutationCtx | CurrentUser | Sequence | Allowance | Files> {
    const ctx = yield* MutationCtx
    const { item, sermon } = yield* find(args.uid)
    // A retry of a finish that went through.
    if (sermon.status !== "recording") return { result: "ok", seq: item.seq, record: yield* asRecord("item", item) }
    const durationSec = Math.ceil(args.durationMs / 1000)
    if (!args.audioFileUid) {
      yield* db(() => ctx.db.patch(sermon._id, { status: "waiting", durationSec }))
      const written = yield* writeItem({ ...sermon, durationSec }, { status: "waiting" })
      return { result: "ok", seq: written.seq, record: yield* asRecord("item", written) }
    }
    const user = yield* userOf(sermon)
    const allowance = yield* Allowance
    // The plan may have changed since `begin` answered: this is where it is reconciled.
    const admitted = yield* Effect.option(allowance.admitSermon(user, durationSec))
    const updated = { ...sermon, durationSec, audio: args.audioFileUid, trial: Option.isSome(admitted) && admitted.value.trial }
    yield* db(() => ctx.db.patch(sermon._id, { durationSec, audio: args.audioFileUid, trial: updated.trial }))
    if (Option.isSome(admitted)) yield* allowance.reserveSermon(updated)
    const written = yield* startWorkflow(updated, Option.isSome(admitted))
    return { result: "ok", seq: written.seq, record: yield* asRecord("item", written) }
  })
})

/** Transcribes a `waiting` sermon with audio now that the plan allows it. */
export const transcribeNow = effectMutation({
  args: Schema.Struct({ uid: Uid }),
  handler: Effect.fn("sermons.transcribeNow")(function*(
    args
  ): Effect.fn.Return<WriteAnswer, InvalidRequest | PlanRequired | AllowanceExhausted, MutationCtx | CurrentUser | Sequence | Allowance | Files> {
    const ctx = yield* MutationCtx
    const { sermon } = yield* find(args.uid)
    if (sermon.status !== "waiting" || !sermon.audio) return yield* new InvalidRequest({ message: "That sermon has no audio waiting to be transcribed." })
    const allowance = yield* Allowance
    const { trial } = yield* allowance.admitSermon(yield* userOf(sermon), sermon.durationSec)
    yield* db(() => ctx.db.patch(sermon._id, { trial }))
    yield* allowance.reserveSermon({ ...sermon, trial })
    const written = yield* startWorkflow({ ...sermon, trial }, true)
    return { result: "ok", seq: written.seq, record: yield* asRecord("item", written) }
  })
})

// ---------------------------------------------------------------------------------------------
// The workflow

const workflow = new WorkflowManager(components.workflow)

export const transcription = workflow.define({
  args: { sermonId: v.id("sermons"), transcribe: v.boolean() },
  handler: async (step, { sermonId, transcribe }) => {
    const audio = await step.runAction(internal.library.audio.join, { sermonId }, { retry: true })
    if (!transcribe || !audio) {
      await step.runMutation(internal.library.sermons.wait, { sermonId })
      return
    }
    const job = await step.runAction(internal.library.sermons.startJob, { audioUrl: audio.url })
    await step.runMutation(internal.library.sermons.started, { sermonId, ...job })
    await step.awaitEvent({ name: "transcript" })
    await step.runAction(internal.library.sermons.keep, { sermonId, assemblyId: job.assemblyId }, { retry: true })
  }
})

/** What `join` tells the workflow: the sermon's `audio` file, or null when nothing was uploaded. */
export const pieces = effectInternalQuery({
  args: { sermonId: v.id("sermons") },
  handler: Effect.fn("sermons.pieces")(function*({ sermonId }) {
    const ctx = yield* QueryCtx
    const sermon = (yield* db(() => ctx.db.get(sermonId)))!
    const item = (yield* db(() => ctx.db.get(sermon.itemId!)))!
    const rows = yield* db(() => ctx.db.query("files").withIndex("by_owner", (q) => q.eq("userId", sermon.userId).eq("ownerUid", item.uid)).collect())
    const audio = rows.find((f) => f.role === "audio")
    if (audio) return { audio: { url: audio.url }, expected: undefined, contentType: audio.contentType, slices: [] }
    // The M4A's slices when the device finished; otherwise the ADTS parts a dead device left.
    const role = sermon.audio ? "uploadPart" : "audioPart"
    const slices = rows
      .filter((f) => f.role === role)
      .sort((a, b) => (a.partIndex ?? 0) - (b.partIndex ?? 0))
      .map((f) => ({ storageId: f.storageId, partIndex: f.partIndex ?? 0 }))
    return { audio: null, expected: sermon.audio, contentType: sermon.audio ? "audio/mp4" : "audio/aac", slices }
  })
})

/** The joined bytes are stored: they become the `audio` file, the parts and slices go, and the item shows it. */
export const joined = effectInternalMutation({
  args: { sermonId: v.id("sermons"), storageId: v.id("_storage"), sha256: v.string() },
  handler: Effect.fn("sermons.joined")(function*({ sermonId, storageId, sha256 }) {
    const ctx = yield* MutationCtx
    const sermon = (yield* db(() => ctx.db.get(sermonId)))!
    const item = (yield* db(() => ctx.db.get(sermon.itemId!)))!
    return yield* asOwner(
      sermon,
      Effect.gen(function*() {
        const files = yield* Files
        const rows = yield* db(() => ctx.db.query("files").withIndex("by_owner", (q) => q.eq("userId", sermon.userId).eq("ownerUid", item.uid)).collect())
        for (const f of rows) if (f.role === "uploadPart" || f.role === "audioPart") yield* files.remove(f.uid)
        const audio = yield* files.attach({ uid: sha256, ownerUid: item.uid, role: "audio", storageId, sha256 })
        yield* db(() => ctx.db.patch(sermon._id, { audio: sha256 }))
        yield* writeItem({ ...sermon, audio: sha256 }, {})
        return { url: audio.url }
      })
    )
  })
})

/** The sermon waits for the device to transcribe it, or for "Transcribe now". */
export const wait = effectInternalMutation({
  args: { sermonId: v.id("sermons") },
  handler: Effect.fn("sermons.wait")(function*({ sermonId }) {
    const ctx = yield* MutationCtx
    const sermon = (yield* db(() => ctx.db.get(sermonId)))!
    yield* db(() => ctx.db.patch(sermon._id, { status: "waiting" }))
    yield* asOwner(sermon, writeItem(sermon, { status: "waiting" }))
  })
})

/** Starts the provider's job on the audio; the per-job secret goes to the provider, only its hash is kept. */
export const startJob = effectInternalAction({
  args: { audioUrl: v.string() },
  handler: Effect.fn("sermons.startJob")(function*({ audioUrl }): Effect.fn.Return<{ assemblyId: string; webhookSecretHash: string }, UpstreamFailed | UpstreamUnavailable, Transcriber> {
    const secret = crypto.randomUUID()
    const assemblyId = yield* (yield* Transcriber).start({ audioUrl, webhook: { url: `${process.env.CONVEX_SITE_URL}/assemblyai`, secret } })
    return { assemblyId, webhookSecretHash: yield* Effect.promise(() => sha256Hex(new TextEncoder().encode(secret))) }
  })
})

export const started = effectInternalMutation({
  args: { sermonId: v.id("sermons"), assemblyId: v.string(), webhookSecretHash: v.string() },
  handler: Effect.fn("sermons.started")(function*({ sermonId, assemblyId, webhookSecretHash }) {
    const ctx = yield* MutationCtx
    const sermon = (yield* db(() => ctx.db.get(sermonId)))!
    const transcribingAt = yield* Clock.currentTimeMillis
    yield* db(() => ctx.db.patch(sermon._id, { status: "transcribing", assemblyId, webhookSecretHash, transcribingAt }))
    yield* asOwner(sermon, writeItem(sermon, { status: "processing" }))
  })
})

/** The job is done: its transcript becomes the item's `text` file, the sermon settles, and the provider's copy goes. */
export const keep = effectInternalAction({
  args: { sermonId: v.id("sermons"), assemblyId: v.string() },
  handler: Effect.fn("sermons.keep")(function*({ sermonId, assemblyId }): Effect.fn.Return<void, UpstreamFailed | UpstreamUnavailable, ActionCtx | Transcriber> {
    const ctx = yield* ActionCtx
    const transcriber = yield* Transcriber
    const job = yield* transcriber.get(assemblyId)
    if (job.status !== "completed") {
      return yield* new UpstreamFailed({ message: job.status === "error" ? job.error : "The transcription isn't finished." })
    }
    const bytes = new TextEncoder().encode(JSON.stringify(job.transcript))
    const sha256 = yield* Effect.promise(() => sha256Hex(bytes))
    const storageId = yield* Effect.promise(() => ctx.storage.store(new Blob([bytes], { type: "application/json" })))
    const transcript = { ...job.transcript, segments: [...job.transcript.segments] }
    const kept: boolean = yield* Effect.promise(() => ctx.runMutation(internal.library.sermons.settle, { sermonId, storageId, sha256, transcript }))
    if (!kept) yield* Effect.promise(() => ctx.storage.delete(storageId))
    yield* transcriber.delete(assemblyId)
  })
})

const excerptOf = (t: Transcript) => t.segments.map((s) => s.text).join(" ").slice(0, EXCERPT)

export const settle = effectInternalMutation({
  args: {
    sermonId: v.id("sermons"),
    storageId: v.id("_storage"),
    sha256: v.string(),
    transcript: v.object(TranscriptFields)
  },
  handler: Effect.fn("sermons.settle")(function*({ sermonId, storageId, sha256, transcript }) {
    const ctx = yield* MutationCtx
    const sermon = (yield* db(() => ctx.db.get(sermonId)))!
    // A step retried after it went through: the first run settled it.
    if (sermon.status !== "transcribing") return false
    const item = (yield* db(() => ctx.db.get(sermon.itemId!)))!
    const audioSeconds = Math.ceil(transcript.durationMs / 1000)
    const doneAt = yield* Clock.currentTimeMillis
    yield* db(() => ctx.db.patch(sermon._id, { status: "done", audioSeconds, doneAt }))
    yield* (yield* Allowance).settleSermon(sermon, audioSeconds)
    yield* asOwner(
      sermon,
      Effect.gen(function*() {
        const files = yield* Files
        const text = yield* files.attach({ uid: sha256, ownerUid: item.uid, role: "text", storageId, sha256 })
        // A Free trial's audio stays on the device that recorded it.
        if (sermon.trial && sermon.audio) yield* files.remove(sermon.audio)
        yield* writeItem({ ...sermon, audioSeconds }, { status: "ready", textFileUid: text.uid, excerpt: excerptOf(transcript) })
      })
    )
    return true
  })
})

/** The workflow ended: a failure or cancellation fails the sermon; success already settled it. */
export const onComplete = effectInternalMutation({
  args: { workflowId: vWorkflowId, result: vResultValidator, context: v.id("sermons") },
  handler: Effect.fn("sermons.onComplete")(function*({ result, context }) {
    if (result.kind === "success") return
    const ctx = yield* MutationCtx
    const sermon = yield* db(() => ctx.db.get(context))
    if (sermon) yield* failSermon(sermon, result.kind === "failed" ? result.error : "The transcription was stopped.")
  })
})

// ---------------------------------------------------------------------------------------------
// The webhook and the crons

/** The provider says a job is done; true when the secret is the job's own. The event wakes the workflow (once). */
export const webhook = effectInternalMutation({
  args: { assemblyId: v.string(), secretHash: v.string() },
  handler: Effect.fn("sermons.webhook")(function*({ assemblyId, secretHash }) {
    const ctx = yield* MutationCtx
    const sermon = yield* db(() => ctx.db.query("sermons").withIndex("by_assembly", (q) => q.eq("assemblyId", assemblyId)).unique())
    if (!sermon?.workflowId || sermon.webhookSecretHash !== secretHash) return false
    yield* wake(ctx, sermon.workflowId)
    return true
  })
})

/** Sends the workflow its `transcript` event; a second delivery finds it sent already, which is fine. */
const wake = (ctx: MutationCtx["Service"] | ActionCtx["Service"], workflowId: string) =>
  Effect.promise(() => sendEvent(ctx, components.workflow, { workflowId: workflowId as WorkflowId, name: "transcript" })).pipe(Effect.ignore)

interface Jobs {
  readonly stuck: Array<Id<"sermons">>
  readonly late: Array<{ assemblyId: string; workflowId: string }>
}

/** Jobs the hourly cron looks at: late ones to ask the provider about, stuck ones to fail. */
export const jobs = effectInternalQuery({
  args: {},
  handler: Effect.fn("sermons.jobs")(function*(): Effect.fn.Return<Jobs, never, QueryCtx> {
    const ctx = yield* QueryCtx
    const now = yield* Clock.currentTimeMillis
    const running = yield* db(() =>
      ctx.db.query("sermons").withIndex("by_status", (q) => q.eq("status", "transcribing").lt("createdAt", now - LATE)).take(200)
    )
    const since = (s: Doc<"sermons">) => now - (s.transcribingAt ?? s.createdAt)
    const mine = running.filter((s) => s.itemId !== undefined && s.workflowId !== undefined && s.assemblyId !== undefined)
    return {
      stuck: mine.filter((s) => since(s) >= STUCK).map((s) => s._id),
      late: mine.filter((s) => since(s) >= LATE && since(s) < STUCK).map((s) => ({ assemblyId: s.assemblyId!, workflowId: s.workflowId! }))
    }
  })
})

/** Fails a sermon whose workflow is still running, and stops the workflow (its `onComplete` then finds the sermon failed). */
export const fail = effectInternalMutation({
  args: { sermonId: v.id("sermons"), error: v.string() },
  handler: Effect.fn("sermons.fail")(function*({ sermonId, error }) {
    const ctx = yield* MutationCtx
    const sermon = yield* db(() => ctx.db.get(sermonId))
    if (!sermon) return
    yield* failSermon(sermon, error)
    if (sermon.workflowId) yield* db(() => cancel(ctx, components.workflow, sermon.workflowId as WorkflowId))
  })
})

/** Hourly: a job whose webhook is late is asked about, and woken if it is done; one stuck for 3 hours fails. */
export const checkJobs = effectInternalAction({
  args: {},
  handler: Effect.fn("sermons.checkJobs")(function*(): Effect.fn.Return<{ failed: number; woken: number }, never, ActionCtx | Transcriber> {
    const ctx = yield* ActionCtx
    const transcriber = yield* Transcriber
    const { stuck, late }: Jobs = yield* Effect.promise(() => ctx.runQuery(internal.library.sermons.jobs, {}))
    for (const sermonId of stuck) {
      yield* Effect.promise(() => ctx.runMutation(internal.library.sermons.fail, { sermonId, error: "Transcription took too long." }))
    }
    let woken = 0
    for (const job of late) {
      const state = yield* transcriber.get(job.assemblyId).pipe(Effect.catch(() => Effect.succeed({ status: "pending" as const })))
      if (state.status === "pending") continue
      yield* wake(ctx, job.workflowId)
      woken++
    }
    return { failed: stuck.length, woken }
  })
})

/**
 * Hourly: a recording with no new part for two hours, and no `finish`, is finished with the parts
 * it has. Their ADTS concatenation becomes the `audio` file (an `audio/aac` file: its timeline is
 * approximate, which is how the item says so), and the sermon waits to be transcribed.
 */
export const finishDead = effectInternalMutation({
  args: {},
  handler: Effect.fn("sermons.finishDead")(function*(): Effect.fn.Return<{ finished: number }, never, MutationCtx | Allowance> {
    const ctx = yield* MutationCtx
    const now = yield* Clock.currentTimeMillis
    const old = yield* db(() =>
      ctx.db.query("sermons").withIndex("by_status", (q) => q.eq("status", "recording").lt("createdAt", now - DEAD)).take(100)
    )
    let finished = 0
    for (const sermon of old) {
      const itemId = sermon.itemId
      if (!itemId) continue
      const item = (yield* db(() => ctx.db.get(itemId)))!
      const parts = yield* db(() => ctx.db.query("files").withIndex("by_owner", (q) => q.eq("userId", sermon.userId).eq("ownerUid", item.uid)).collect())
      const newest = Math.max(sermon.createdAt, ...parts.filter((f) => f.role === "audioPart").map((f) => f._creationTime))
      if (newest > now - DEAD) continue
      yield* asOwner(sermon, startWorkflow(sermon, false))
      finished++
    }
    return { finished }
  })
})
