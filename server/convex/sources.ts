/**
 * The outside sources (docs/plans/m7-outside-sources.md, "Building and loading"): the loader's
 * three doors, each proven by the server secret, and the passage lookup the Commentary section and
 * Ask's `commentaries` tool read.
 *
 * A load writes a work's entries under a new `version`, invisible to readers until `commit` flips
 * the work's row to it in one write; `purge` then removes the previous version's rows in steps.
 * So a reload never shows a reader a half-loaded or half-deleted work.
 */
import { v } from "convex/values"
import { Effect, Schema } from "effect"
import type { Doc, Id } from "./_generated/dataModel.js"
import { InvalidRequest } from "../shared/Errors.js"
import { effectQuery, effectServerMutation, effectServerQuery } from "./lib/effect.js"
import { STEP } from "./lib/purge.js"
import { MutationCtx, QueryCtx } from "./lib/services.js"
import { splitRange } from "./lib/verses.js"

const range = v.object({ from: v.number(), to: v.number() })

const entry = v.object({
  key: v.string(),
  from: v.optional(v.number()),
  to: v.optional(v.number()),
  term: v.optional(v.string()),
  heading: v.optional(v.string()),
  text: v.string(),
  cites: v.optional(v.array(range))
})

const kind = v.union(v.literal("commentary"), v.literal("dictionary"), v.literal("topical"), v.literal("confession"))

/** What a reader gets for an entry: no Convex ids, so a citation names the work and its key. */
export interface SourceEntry {
  readonly work: string
  readonly key: string
  readonly from?: number
  readonly to?: number
  readonly term?: string
  readonly heading?: string
  readonly text: string
  readonly cites?: ReadonlyArray<{ readonly from: number; readonly to: number }>
}

const entryOf = (row: Doc<"sourceEntries">): SourceEntry => ({
  work: row.work,
  key: row.key,
  ...(row.from === undefined ? {} : { from: row.from, to: row.to }),
  ...(row.term === undefined ? {} : { term: row.term }),
  ...(row.heading === undefined ? {} : { heading: row.heading }),
  text: row.text,
  ...(row.cites === undefined ? {} : { cites: row.cites })
})

/**
 * One batch of a work's entries under `version`, with their anchor rows: a commentary entry is
 * anchored on its range, a confession section on each of its proof texts, and a dictionary's or
 * topical index's entry on nothing (its door is its term).
 */
export const load = effectServerMutation({
  args: { work: v.string(), version: v.string(), kind, entries: v.array(entry) },
  handler: Effect.fn("sources.load")(function*(args) {
    const ctx = yield* MutationCtx
    for (const e of args.entries) {
      if ((e.from === undefined) !== (e.to === undefined)) {
        return yield* new InvalidRequest({ message: `${args.work} ${e.key} has one end of a range.` })
      }
      const ranges = e.from !== undefined ? [{ from: e.from, to: e.to! }] : args.kind === "confession" ? e.cites ?? [] : []
      const spans = yield* Effect.forEach(ranges, (r) => splitRange(r.from, r.to))
      const seen = new Set<string>()
      yield* Effect.promise(async () => {
        const entryId = await ctx.db.insert("sourceEntries", { work: args.work, version: args.version, ...e })
        for (const s of spans.flat()) {
          const id = `${s.from}-${s.to}`
          if (seen.has(id)) continue
          seen.add(id)
          await ctx.db.insert("sourceAnchors", { work: args.work, version: args.version, ...s, entryId })
        }
      })
    }
    return { loaded: args.entries.length }
  })
})

/** Makes `version` the one readers see for the work, writing its row (one write, so the swap is atomic). */
export const commit = effectServerMutation({
  args: {
    key: v.string(),
    title: v.string(),
    author: v.string(),
    year: v.number(),
    tradition: v.string(),
    kind,
    licence: v.string(),
    version: v.string()
  },
  handler: Effect.fn("sources.commit")(function*(work) {
    const ctx = yield* MutationCtx
    yield* Effect.promise(async () => {
      const row = await ctx.db.query("sourceWorks").withIndex("by_key", (q) => q.eq("key", work.key)).unique()
      if (row === null) await ctx.db.insert("sourceWorks", work)
      else await ctx.db.replace(row._id, work)
    })
  })
})

/** Deletes up to STEP of a work's rows under `version` (a superseded or abandoned load); `more` when some remain. */
export const purge = effectServerMutation({
  args: { work: v.string(), version: v.string() },
  handler: Effect.fn("sources.purge")(function*(args) {
    const ctx = yield* MutationCtx
    const active = yield* Effect.promise(() => ctx.db.query("sourceWorks").withIndex("by_key", (q) => q.eq("key", args.work)).unique())
    if (active?.version === args.version) {
      return yield* new InvalidRequest({ message: `${args.work} ${args.version} is the version readers see.` })
    }
    return yield* Effect.promise(async () => {
      let budget = STEP
      for (const table of ["sourceAnchors", "sourceEntries"] as const) {
        const rows = await ctx.db.query(table).withIndex("by_work_version", (q) => q.eq("work", args.work).eq("version", args.version)).take(budget + 1)
        for (const row of rows.slice(0, budget)) await ctx.db.delete(row._id)
        budget -= Math.min(rows.length, budget)
        if (budget === 0) return { more: true }
      }
      return { more: false }
    })
  })
})

/** The works readers see and their versions, so the loader skips an unchanged file and purges the one it replaced. */
export const works = effectServerQuery({
  args: {},
  handler: Effect.fn("sources.works")(function*() {
    const ctx = yield* QueryCtx
    const rows = yield* Effect.promise(() => ctx.db.query("sourceWorks").collect())
    return rows.map((w) => ({ key: w.key, version: w.version }))
  })
})

export const PassageArgs = Schema.Struct({
  from: Schema.Int,
  to: Schema.Int,
  /** The works to read; every work when absent. */
  works: Schema.optionalKey(Schema.Array(Schema.String))
})

/**
 * Every committed entry whose range (or proof text) overlaps the passage, including one that starts
 * in an earlier chapter, in canon order of the range and then the work. The anchor index is read
 * once per chapter the passage touches.
 */
export const passage = effectQuery({
  args: PassageArgs,
  empty: () => Effect.succeed<Array<SourceEntry>>([]),
  handler: Effect.fn("sources.passage")(function*(args) {
    const ctx = yield* QueryCtx
    const spans = yield* splitRange(args.from, args.to)
    return yield* Effect.promise(async () => {
      const works = await ctx.db.query("sourceWorks").collect()
      const active = new Map(works.filter((w) => args.works === undefined || args.works.includes(w.key)).map((w) => [w.key, w.version]))
      const ids = new Set<Id<"sourceEntries">>()
      for (const s of spans) {
        const rows = await ctx.db
          .query("sourceAnchors")
          .withIndex("by_chapter", (q) => q.eq("book", s.book).eq("chapter", s.chapter))
          .filter((q) => q.and(q.lte(q.field("from"), s.to), q.gte(q.field("to"), s.from)))
          .collect()
        for (const a of rows) if (active.get(a.work) === a.version) ids.add(a.entryId)
      }
      const entries: Array<SourceEntry> = []
      for (const id of ids) {
        const row = await ctx.db.get(id)
        if (row !== null) entries.push(entryOf(row))
      }
      return entries.sort((a, b) => (a.from ?? 0) - (b.from ?? 0) || (a.to ?? 0) - (b.to ?? 0) || a.work.localeCompare(b.work) || a.key.localeCompare(b.key))
    })
  })
})
