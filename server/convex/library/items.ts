/**
 * An item's details: a sermon's title, speaker, church and times, and which sermon a slide or page
 * belongs to and where in it (docs/plans/library.md, "Sermons"). A note's text and anchors are its
 * own write (`notes.upsert`); this never touches them. A stale `baseSeq` keeps the server's and
 * answers `conflict`, as for a chat.
 */
import { Clock, Effect, Schema } from "effect"
import type { Doc } from "../_generated/dataModel.js"
import { InvalidRequest } from "../../shared/Errors.js"
import { type ChapterItem, Item, type ItemText, type WriteAnswer } from "../../shared/records.js"
import { decideByUid } from "../lib/conflict.js"
import { effectMutation, effectQuery } from "../lib/effect.js"
import { Sequence } from "../lib/sequence.js"
import { CurrentUser, MutationCtx, QueryCtx } from "../lib/services.js"
import { stamp } from "./chunk.js"
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

// ---------------------------------------------------------------------------------------------
// Reading an item, for Ask's `read_item` and the reader's margin (docs/plans/library.md, "Search and Ask")

/** About how many words `read` answers at most, as `read_sermon` did. */
const WORDS = 2_500
const DEFAULT_MINUTES = 10

export const ReadArgs = Schema.Struct({
  uid: Uid,
  /** For a sermon: where the window starts, in milliseconds (the start, if absent). */
  fromMs: Schema.optionalKey(Schema.Int.check(Schema.isGreaterThanOrEqualTo(0))),
  /** For a sermon: how long a window, in minutes. */
  minutes: Schema.optionalKey(Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 60 }))),
  /** For a photographed item: one page of it. */
  page: Schema.optionalKey(Schema.Int.check(Schema.isGreaterThanOrEqualTo(1)))
})

const words = (text: string) => text.split(/\s+/).filter((w) => w.length > 0).length

/**
 * An item's text from its chunks: a note whole, a sermon as a window of its paragraphs by time,
 * each stamped, capped at about WORDS; `null` for an item that isn't in the library.
 */
export const read = effectQuery({
  args: ReadArgs,
  empty: () => Effect.succeed<ItemText | null>(null),
  handler: Effect.fn("items.read")(function*(args): Effect.fn.Return<ItemText | null, never, QueryCtx | CurrentUser> {
    const ctx = yield* QueryCtx
    const { userId } = yield* CurrentUser
    const item = yield* Effect.promise(() => ctx.db.query("items").withIndex("by_user_uid", (q) => q.eq("userId", userId).eq("uid", args.uid)).unique())
    if (!item || item.deletedAt !== undefined) return null
    const rows = yield* Effect.promise(() => ctx.db.query("chunks").withIndex("by_item", (q) => q.eq("itemId", item._id)).collect())
    const chunks = rows.filter((c) => args.page === undefined || c.page === args.page).sort((a, b) => a.ordinal - b.ordinal)
    const timed = item.kind === "sermon"
    const from = args.fromMs ?? 0
    const to = from + (args.minutes ?? DEFAULT_MINUTES) * 60_000
    const window = timed ? chunks.filter((c) => c.endMs !== undefined && c.startMs !== undefined && c.endMs > from && c.startMs < to) : chunks
    const taken: Array<typeof window[number]> = []
    let count = 0
    for (const c of window) {
      if (taken.length > 0 && count + words(c.text) > WORDS) break
      taken.push(c)
      count += words(c.text)
    }
    const last = taken.at(-1)
    const truncated = taken.length < window.length || (timed && chunks.some((c) => c.startMs !== undefined && c.startMs >= to))
    return {
      uid: item.uid,
      kind: item.kind,
      title: item.title,
      text: taken.map((c) => (timed && c.startMs !== undefined ? `${stamp(c.startMs)} ${c.text}` : c.text)).join("\n\n"),
      ...(timed ? { fromMs: taken[0]?.startMs ?? from } : {}),
      ...(timed && last?.endMs !== undefined ? { toMs: last.endMs } : {}),
      truncated
    }
  })
})

export const ForChapterArgs = Schema.Struct({
  book: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 65 })),
  chapter: Schema.Int.check(Schema.isGreaterThanOrEqualTo(1))
})

/**
 * The anchors on a chapter with their items, for the margin and "In your library": one entry per
 * anchor, in verse order, each with the anchor's whole range (one that starts in the chapter
 * before says so; the index rows hold only this chapter's part of it).
 */
export const forChapter = effectQuery({
  args: ForChapterArgs,
  empty: () => Effect.succeed<Array<ChapterItem>>([]),
  handler: Effect.fn("items.forChapter")(function*({ book, chapter }): Effect.fn.Return<Array<ChapterItem>, never, QueryCtx | CurrentUser> {
    const ctx = yield* QueryCtx
    const { userId } = yield* CurrentUser
    const anchors = yield* Effect.promise(() =>
      ctx.db.query("anchors").withIndex("by_user_chapter", (q) => q.eq("userId", userId).eq("book", book).eq("chapter", chapter)).collect()
    )
    const items = new Map<string, Doc<"items"> | null>()
    const out: Array<ChapterItem> = []
    for (const a of anchors) {
      if (!items.has(a.itemId)) items.set(a.itemId, yield* Effect.promise(() => ctx.db.get(a.itemId)))
      const item = items.get(a.itemId)
      if (!item || item.deletedAt !== undefined) continue
      const whole = item.anchors.find((x) => x.from <= a.from && x.to >= a.to && x.origin === a.origin && x.atMs === a.atMs && x.page === a.page) ?? a
      out.push({
        uid: item.uid,
        kind: item.kind,
        title: item.title,
        from: whole.from,
        to: whole.to,
        origin: a.origin,
        ...(a.atMs !== undefined ? { atMs: a.atMs } : {}),
        ...(a.page !== undefined ? { page: a.page } : {})
      })
    }
    return out.sort((x, y) => x.from - y.from || x.to - y.to)
  })
})
