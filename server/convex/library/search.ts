/**
 * `library.search` (docs/plans/library.md, "Search and Ask"): the chunks anchored to the passage
 * asked about, then the text index, fused by rank (library/rank.ts) with anchored and focus
 * chunks boosted, grouped by item. An action, not a query: it merges several reads, so it is
 * neither reactive nor cached. It searches reviewed and unreviewed chunks alike and says which.
 */
import { v } from "convex/values"
import { Effect, Schema } from "effect"
import { internal } from "../_generated/api.js"
import type { Doc, Id } from "../_generated/dataModel.js"
import type { InvalidRequest } from "../../shared/Errors.js"
import { Item, type SearchHit } from "../../shared/records.js"
import { effectAction, effectInternalQuery } from "../lib/effect.js"
import { ActionCtx, CurrentUser, QueryCtx } from "../lib/services.js"
import { splitRange } from "../lib/verses.js"
import { type Candidate, fuse } from "./rank.js"

const LIMIT = 20
const MAX_LIMIT = 50
/** How many hits one item may take, so one sermon doesn't fill every slot. */
const PER_ITEM = 3

export const SearchArgs = Schema.Struct({
  query: Schema.String,
  /** The passage being read: chunks anchored to any verse in it come first. */
  passage: Schema.optionalKey(Schema.Struct({ from: Schema.Int, to: Schema.Int })),
  kinds: Schema.optionalKey(Schema.Array(Item.fields.kind)),
  /** A speaker's or church's name, matched case-insensitively. */
  source: Schema.optionalKey(Schema.String),
  limit: Schema.optionalKey(Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: MAX_LIMIT })))
})

interface Hit extends Candidate {
  readonly hit: SearchHit
}

/** A chunk as a hit, with what the apps show of its item. */
const hitOf = (chunk: Doc<"chunks">, item: Doc<"items">, anchored: boolean): Hit => {
  const source = [item.details?.speaker, item.details?.from].filter((s): s is string => !!s).join(", ")
  return {
    key: chunk._id,
    itemKey: item._id,
    boosted: anchored || chunk.focus,
    hit: {
      itemUid: item.uid,
      kind: item.kind,
      title: item.title,
      ...(source ? { source } : {}),
      date: item.details?.startedAt ?? item.capturedAt,
      text: chunk.text,
      ...(chunk.startMs !== undefined ? { startMs: chunk.startMs } : {}),
      ...(chunk.endMs !== undefined ? { endMs: chunk.endMs } : {}),
      ...(chunk.page !== undefined ? { page: chunk.page } : {}),
      anchored,
      unreviewed: chunk.unreviewed
    }
  }
}

/** The two ranked lists, each best first, for the action to fuse. */
export const gather = effectInternalQuery({
  args: {
    userId: v.id("users"),
    query: v.string(),
    passage: v.optional(v.object({ from: v.number(), to: v.number() })),
    kinds: v.optional(v.array(v.string())),
    source: v.optional(v.string()),
    limit: v.number()
  },
  handler: Effect.fn("search.gather")(function*({ userId, query, passage, kinds, source, limit }): Effect.fn.Return<{ anchored: Array<Hit>; text: Array<Hit> }, InvalidRequest, QueryCtx> {
    const ctx = yield* QueryCtx
    const wanted = source?.trim().toLowerCase()
    const items = new Map<Id<"items">, Doc<"items"> | null>()
    /** The chunk's item, if it is live and what was asked for. */
    const itemOf = async (itemId: Id<"items">) => {
      if (!items.has(itemId)) {
        const item = await ctx.db.get(itemId)
        const kept = item !== null &&
          item.deletedAt === undefined &&
          (!kinds || kinds.includes(item.kind)) &&
          (!wanted || [item.details?.speaker, item.details?.from].some((s) => s?.toLowerCase().includes(wanted)))
        items.set(itemId, kept ? item : null)
      }
      return items.get(itemId)!
    }
    const anchored: Array<Hit> = []
    if (passage) {
      const spans = yield* splitRange(passage.from, passage.to)
      const seen = new Set<string>()
      const found: Array<{ anchor: Doc<"anchors">; item: Doc<"items">; chunk: Doc<"chunks"> }> = []
      for (const span of spans) {
        const rows = yield* Effect.promise(() =>
          ctx.db.query("anchors").withIndex("by_user_chapter", (q) => q.eq("userId", userId).eq("book", span.book).eq("chapter", span.chapter)).collect()
        )
        for (const anchor of rows) {
          if (anchor.to < span.from || anchor.from > span.to) continue
          const item = yield* Effect.promise(() => itemOf(anchor.itemId))
          if (!item) continue
          const chunks = yield* Effect.promise(() => ctx.db.query("chunks").withIndex("by_item", (q) => q.eq("itemId", anchor.itemId)).collect())
          // At a time, the paragraph then being said; on a page, that page's; otherwise the whole item.
          const at = anchor.atMs
          const picked = at !== undefined
            ? [chunks.filter((c) => c.startMs !== undefined && c.endMs !== undefined).sort((a, b) => distance(a, at) - distance(b, at))[0]]
            : anchor.page !== undefined
            ? chunks.filter((c) => c.page === anchor.page)
            : chunks
          for (const chunk of picked) {
            if (!chunk || seen.has(chunk._id)) continue
            seen.add(chunk._id)
            found.push({ anchor, item, chunk })
          }
        }
      }
      // The person's own anchors before a model's; then the newest item first.
      found.sort((a, b) => (a.anchor.origin === "model" ? 1 : 0) - (b.anchor.origin === "model" ? 1 : 0) || b.item.capturedAt - a.item.capturedAt)
      anchored.push(...found.map((f) => hitOf(f.chunk, f.item, true)))
    }
    const text: Array<Hit> = []
    if (query.trim() !== "") {
      const rows = yield* Effect.promise(() =>
        ctx.db.query("chunks").withSearchIndex("text", (q) => q.search("text", query).eq("userId", userId)).take(limit * 3)
      )
      for (const chunk of rows) {
        const item = yield* Effect.promise(() => itemOf(chunk.itemId))
        if (item) text.push(hitOf(chunk, item, anchored.some((h) => h.key === chunk._id)))
      }
    }
    return { anchored, text }
  })
})

/** How far a time is from a chunk's span: zero inside it. */
const distance = (c: Doc<"chunks">, at: number) => Math.max(0, c.startMs! - at, at - c.endMs!)

export const search = effectAction({
  args: SearchArgs,
  empty: () => Effect.succeed<Array<SearchHit>>([]),
  handler: Effect.fn("search.search")(function*(args): Effect.fn.Return<Array<SearchHit>, InvalidRequest, ActionCtx | CurrentUser> {
    const ctx = yield* ActionCtx
    const { userId } = yield* CurrentUser
    const limit = args.limit ?? LIMIT
    const lists: { anchored: Array<Hit>; text: Array<Hit> } = yield* Effect.promise(() =>
      ctx.runQuery(internal.library.search.gather, {
        userId,
        query: args.query,
        ...(args.passage ? { passage: args.passage } : {}),
        ...(args.kinds ? { kinds: [...args.kinds] } : {}),
        ...(args.source ? { source: args.source } : {}),
        limit
      })
    )
    return fuse([lists.anchored, lists.text], limit, PER_ITEM).map((h) => h.hit)
  })
})
