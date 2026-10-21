/**
 * The digest step (docs/plans/library.md, "The digest step"): after a sermon is transcribed, and
 * on demand for the sermons already in a library, a model reads the transcript once and the
 * references it cites become the item's `model` anchors at their times, with an outline and a
 * summary on the item. A reference that isn't on the grid is dropped and logged, never guessed.
 * The text itself is searchable before this runs (library/chunk.ts, at `sermons.settle`).
 */
import { v } from "convex/values"
import { Clock, Effect, Schema } from "effect"
import { internal } from "../_generated/api.js"
import type { Doc, Id } from "../_generated/dataModel.js"
import { InvalidRequest, type UpstreamFailed, type UpstreamUnavailable } from "../../shared/Errors.js"
import { Transcript, TranscriptFields } from "../../shared/records.js"
import { loadModel } from "../lib/accounts.js"
import { Anchors } from "../lib/anchors.js"
import { replaceChunks } from "../lib/chunks.js"
import { Digest, type Reference } from "../lib/digest.js"
import { effectInternalAction, effectInternalMutation, effectInternalQuery, userServices } from "../lib/effect.js"
import { type TokenUsage, tokenCost } from "../lib/pricing.js"
import { parseReference } from "../lib/references.js"
import { Sequence } from "../lib/sequence.js"
import { ActionCtx, MutationCtx, QueryCtx } from "../lib/services.js"
import { chunkNote, chunkTranscript, stamp } from "./chunk.js"

const db = <A>(f: () => Promise<A>) => Effect.promise(f)

/** The transcript as the model reads it: one line a segment, stamped. */
export const lines = (t: Transcript) => t.segments.map((s) => `${stamp(s.startMs)} ${s.text}`).join("\n")

/** What `run` needs from the database: the item, where its transcript is, and the `digest` model. */
export const source = effectInternalQuery({
  args: { itemId: v.id("items") },
  handler: Effect.fn("digest.source")(function*({ itemId }) {
    const ctx = yield* QueryCtx
    const item = yield* db(() => ctx.db.get(itemId))
    if (!item || item.deletedAt !== undefined || item.kind !== "sermon" || !item.textFileUid) return null
    const file = yield* db(() => ctx.db.query("files").withIndex("by_user_uid", (q) => q.eq("userId", item.userId).eq("uid", item.textFileUid!)).unique())
    const model = yield* db(() => loadModel(ctx, "digest"))
    if (!file || !model) return null
    return { title: item.title, storageId: file.storageId, model }
  })
})

/**
 * Digests the sermon item, or does nothing for an item with no transcript. With `rechunk`, its
 * chunks are rebuilt from the transcript first (the backfill; `sermons.settle` does it otherwise).
 */
export const run = effectInternalAction({
  args: { itemId: v.id("items"), rechunk: v.optional(v.boolean()) },
  handler: Effect.fn("digest.run")(function*({ itemId, rechunk }): Effect.fn.Return<{ digested: boolean }, UpstreamFailed | UpstreamUnavailable, ActionCtx | Digest> {
    const ctx = yield* ActionCtx
    const src = yield* db(() => ctx.runQuery(internal.library.digest.source, { itemId }))
    if (!src) return { digested: false }
    const blob = yield* db(() => ctx.storage.get(src.storageId))
    if (!blob) return { digested: false }
    const transcript = yield* Schema.decodeUnknownEffect(Transcript)(JSON.parse(yield* db(() => blob.text()))).pipe(Effect.orDie)
    if (rechunk) yield* db(() => ctx.runMutation(internal.library.digest.rechunk, { itemId, transcript: { ...transcript, segments: [...transcript.segments] } }))
    const startedAt = yield* Clock.currentTimeMillis
    const output = yield* (yield* Digest).digest({ model: src.model, title: src.title, text: lines(transcript) })
    const durationMs = (yield* Clock.currentTimeMillis) - startedAt
    yield* db(() =>
      ctx.runMutation(internal.library.digest.apply, {
        itemId,
        references: [...output.references],
        outline: output.outline,
        summary: output.summary,
        usage: output.usage,
        durationMs
      })
    )
    return { digested: true }
  })
})

const anchorsOf = Effect.fnUntraced(function*(references: ReadonlyArray<Reference>) {
  const anchors: Array<Doc<"items">["anchors"][number]> = []
  for (const r of references) {
    const range = parseReference(r.ref)
    if (!range) {
      yield* Effect.logWarning("the digest named a reference that isn't on the grid", { ref: r.ref })
      continue
    }
    anchors.push({ from: range.from, to: range.to, origin: "model", ...(r.atMs !== undefined ? { atMs: r.atMs } : {}), ...(r.page !== undefined ? { page: r.page } : {}) })
  }
  return anchors
})

/** The digest's references become the item's `model` anchors (its own are kept), with the outline and summary; the ledger records the cost. */
export const apply = effectInternalMutation({
  args: {
    itemId: v.id("items"),
    references: v.array(v.object({ ref: v.string(), atMs: v.optional(v.number()), page: v.optional(v.number()) })),
    outline: v.string(),
    summary: v.string(),
    usage: v.object({ inputTokens: v.number(), outputTokens: v.number(), cacheReadTokens: v.number(), cacheWriteTokens: v.number() }),
    durationMs: v.number()
  },
  handler: Effect.fn("digest.apply")(function*({ itemId, references, outline, summary, usage, durationMs }) {
    const ctx = yield* MutationCtx
    const item = yield* db(() => ctx.db.get(itemId))
    if (!item || item.deletedAt !== undefined) return
    const user = (yield* db(() => ctx.db.get(item.userId)))!
    const model = yield* db(() => loadModel(ctx, "digest"))
    if (!model) return yield* Effect.die(new Error("the digest model isn't seeded; run admin:seed"))
    const anchors = [...item.anchors.filter((a) => a.origin !== "model"), ...(yield* anchorsOf(references))]
    const now = yield* Clock.currentTimeMillis
    yield* Effect.gen(function*() {
      const seq = yield* (yield* Sequence).next
      yield* db(() => ctx.db.patch(item._id, { anchors, outline, summary, seq, updatedAt: now }))
      yield* (yield* Anchors).rebuild(item._id, anchors).pipe(Effect.orDie)
    }).pipe(Effect.provide(userServices(user)))
    yield* db(() =>
      ctx.db.insert("ledger", {
        userId: item.userId,
        kind: "digest",
        model: model.gatewayModel,
        inputTokens: usage.inputTokens,
        outputTokens: usage.outputTokens,
        cacheReadTokens: usage.cacheReadTokens,
        cacheWriteTokens: usage.cacheWriteTokens,
        costMicros: tokenCost(usage as TokenUsage, model.prices),
        durationMs,
        status: "ok",
        at: now
      })
    )
  })
})

/** Rebuilds an item's chunks: a note's from its text, a sermon's from the transcript given. */
export const rechunk = effectInternalMutation({
  args: { itemId: v.id("items"), transcript: v.optional(v.object(TranscriptFields)) },
  handler: Effect.fn("digest.rechunk")(function*({ itemId, transcript }) {
    const ctx = yield* MutationCtx
    const item = yield* db(() => ctx.db.get(itemId))
    if (!item || item.deletedAt !== undefined) return
    if (item.kind === "note") return yield* replaceChunks(ctx, item, chunkNote(item.text ?? ""))
    if (item.kind === "sermon" && transcript) return yield* replaceChunks(ctx, item, chunkTranscript(transcript, item.unreviewed))
    return yield* new InvalidRequest({ message: `A ${item.kind} isn't chunked here.` })
  })
})

/** Every live note and sermon, for the backfill. */
export const items = effectInternalQuery({
  args: {},
  handler: Effect.fn("digest.items")(function*() {
    const ctx = yield* QueryCtx
    const rows = yield* db(() => ctx.db.query("items").collect())
    return rows.filter((i) => i.deletedAt === undefined && (i.kind === "note" || (i.kind === "sermon" && i.textFileUid))).map((i) => ({ itemId: i._id, kind: i.kind }))
  })
})

/** One-off, from the dashboard: chunks every note and sermon again, and digests every sermon. */
export const backfill = effectInternalAction({
  args: {},
  handler: Effect.fn("digest.backfill")(function*(): Effect.fn.Return<{ chunked: number; digested: number }, never, ActionCtx> {
    const ctx = yield* ActionCtx
    const all: Array<{ itemId: Id<"items">; kind: string }> = yield* db(() => ctx.runQuery(internal.library.digest.items, {}))
    let digested = 0
    for (const { itemId, kind } of all) {
      if (kind === "note") {
        yield* db(() => ctx.runMutation(internal.library.digest.rechunk, { itemId }))
        continue
      }
      const done: { digested: boolean } = yield* Effect.tryPromise({
        try: () => ctx.runAction(internal.library.digest.run, { itemId, rechunk: true }),
        catch: (e) => e
      }).pipe(Effect.catch((e) => Effect.as(Effect.logError("a sermon's digest failed in the backfill", { itemId, error: String(e) }), { digested: false })))
      if (done.digested) digested++
    }
    return { chunked: all.length, digested }
  })
})
