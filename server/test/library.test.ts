import { describe, expect, it } from "vitest"
import { PAGE_RECORDS } from "../convex/library/sync.js"
import { api } from "../convex/_generated/api.js"
import { makeConvex, type TestConvex } from "./harness.js"

const JOHN_3_16 = 42_003_016
const ROMANS_8_28 = 44_008_028

const as = (t: TestConvex, subject: string) =>
  t.withIdentity({ subject, issuer: "https://clerk.test", tokenIdentifier: `https://clerk.test|${subject}` })

const errorOf = async (p: Promise<unknown>): Promise<{ type: string; message: string }> => {
  try {
    await p
  } catch (e) {
    return (e as { data: { type: string; message: string } }).data
  }
  throw new Error("expected a failure")
}

describe("library.highlights (Effect inside Convex)", () => {
  it("writes, lists and clears a highlight with a rising sequence", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    expect(await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "yellow" })).toEqual({ result: "ok", seq: 1 })
    expect(await ann.mutation(api.library.highlights.set, { verse: ROMANS_8_28, color: "blue" })).toEqual({ result: "ok", seq: 2 })
    expect(await ann.query(api.library.highlights.list, {})).toEqual([
      { uid: "h-42003016", seq: 1, verse: JOHN_3_16, color: "yellow" },
      { uid: "h-44008028", seq: 2, verse: ROMANS_8_28, color: "blue" }
    ])
    expect(await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: null, baseSeq: 1 })).toEqual({ result: "ok", seq: 3 })
    expect(await ann.query(api.library.highlights.list, {})).toEqual([{ uid: "h-44008028", seq: 2, verse: ROMANS_8_28, color: "blue" }])
    // Clearing what isn't there is a no-op, not a write.
    expect(await ann.mutation(api.library.highlights.set, { verse: 1_001_001, color: null })).toEqual({ result: "ok", seq: 0 })
    const rows = await t.run((ctx) => ctx.db.query("highlights").collect())
    expect(rows.map((r) => [r.verse, r.seq, r.deletedAt !== undefined])).toEqual([
      [JOHN_3_16, 3, true],
      [ROMANS_8_28, 2, false]
    ])
  })

  it("dates a record when it is made: by the device's own time if given, kept on edits, set again when it comes back", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    const created = async () => (await t.run((ctx) => ctx.db.query("highlights").first()))!.createdAt
    const MIGRATED = Date.UTC(2019, 4, 1)
    await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "yellow", createdAt: MIGRATED })
    expect(await created()).toBe(MIGRATED)
    await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "green", baseSeq: 1, createdAt: Date.UTC(2026, 0, 1) })
    expect(await created()).toBe(MIGRATED)
    await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: null, baseSeq: 2 })
    expect(await created()).toBe(MIGRATED)
    const before = Date.now()
    await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "pink", baseSeq: 3 })
    expect(await created()).toBeGreaterThanOrEqual(before)
  })

  it("keeps the server's copy when the writer's baseSeq is stale", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "yellow" }) // seq 1
    await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "green", baseSeq: 1 }) // seq 2, from device A
    // Device B edited from the copy at seq 1 while offline.
    expect(await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "pink", baseSeq: 1 })).toEqual({
      result: "conflict",
      seq: 2,
      color: "green",
      createdAt: expect.any(Number)
    })
    expect(await ann.query(api.library.highlights.list, {})).toEqual([{ uid: "h-42003016", seq: 2, verse: JOHN_3_16, color: "green" }])
    // A write without baseSeq (a first write from a device with no copy) still wins.
    expect(await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "pink" })).toEqual({ result: "ok", seq: 3 })
  })

  it("never shows one user's records to another", async () => {
    const t = await makeConvex()
    await as(t, "user_ann").mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "yellow" })
    expect(await as(t, "user_bob").query(api.library.highlights.list, {})).toEqual([])
    expect(await as(t, "user_bob").mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "blue" })).toEqual({ result: "ok", seq: 1 })
    expect(await as(t, "user_ann").query(api.library.highlights.list, {})).toEqual([{ uid: "h-42003016", seq: 1, verse: JOHN_3_16, color: "yellow" }])
  })

  it("refuses callers without a Linger user identity", async () => {
    const t = await makeConvex()
    expect((await errorOf(t.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "yellow" }))).type).toBe("unauthenticated")
    expect((await errorOf(as(t, "org_church").query(api.library.highlights.list, {}))).type).toBe("unauthenticated")
    // A query before any mutation created the user's row sees an empty library, not an error.
    expect(await as(t, "user_new").query(api.library.highlights.list, {})).toEqual([])
    expect(await t.run((ctx) => ctx.db.query("users").collect())).toEqual([])
  })

  it("answers a bad argument shape or an impossible verse as invalid_request, through the shared error classes", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    const shape = await errorOf(ann.mutation(api.library.highlights.set, { verse: "John 3:16", color: "yellow" } as never))
    expect(shape.type).toBe("invalid_request")
    expect(shape.message).toMatch(/arguments are invalid/)
    const grid = await errorOf(ann.mutation(api.library.highlights.set, { verse: 42_022_001, color: "yellow" }))
    expect(grid.type).toBe("invalid_request")
    expect(grid.message).toMatch(/isn't a verse/)
    const colour = await errorOf(ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "mauve" } as never))
    expect(colour.type).toBe("invalid_request")
  })
})

describe("library.bookmarks", () => {
  it("keeps the server's copy when the writer's baseSeq is stale", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    expect(await ann.mutation(api.library.bookmarks.set, { verse: JOHN_3_16, label: "Gospel" })).toEqual({ result: "ok", seq: 1 })
    expect(await ann.mutation(api.library.bookmarks.set, { verse: JOHN_3_16, label: "The gospel", baseSeq: 1 })).toEqual({ result: "ok", seq: 2 })
    expect(await ann.mutation(api.library.bookmarks.set, { verse: JOHN_3_16, label: null, baseSeq: 1 })).toEqual({
      result: "conflict",
      seq: 2,
      label: "The gospel",
      createdAt: expect.any(Number)
    })
    const rows = await t.run((ctx) => ctx.db.query("bookmarks").collect())
    expect(rows.map((r) => [r.verse, r.seq, r.label, r.deletedAt])).toEqual([[JOHN_3_16, 2, "The gospel", undefined]])
  })
})

describe("library.sync.changes", () => {
  type Page = Extract<Awaited<ReturnType<typeof pull>>, { records: unknown }>
  const pull = (t: TestConvex, subject: string, after: number) => as(t, subject).query(api.library.sync.changes, { after })
  const page = async (t: TestConvex, subject: string, after: number): Promise<Page> => {
    const p = await pull(t, subject, after)
    if (!("records" in p)) throw new Error(`expected a page, got ${JSON.stringify(p)}`)
    return p
  }

  it("pages every type by seq without skipping, tombstones included, and never leaks ids", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    // More writes than one page holds, alternating types so a page boundary falls between them.
    const verses = Array.from({ length: 260 }, (_, i) => 40_001_001 + i % 25 + Math.floor(i / 25) * 1_000)
    for (const verse of verses) {
      await ann.mutation(api.library.highlights.set, { verse, color: "yellow" })
      await ann.mutation(api.library.bookmarks.set, { verse, label: `v${verse}` })
    }
    // An edit and a delete move two early records to the end.
    await ann.mutation(api.library.highlights.set, { verse: verses[0]!, color: "blue" })
    await ann.mutation(api.library.bookmarks.set, { verse: verses[1]!, label: null })
    const total = verses.length * 2 + 2

    const first = await page(t, "user_ann", 0)
    expect(first.records).toHaveLength(PAGE_RECORDS)
    expect(first.seq).toBe(first.records.at(-1)!.seq)
    const second = await page(t, "user_ann", first.seq)
    const third = await page(t, "user_ann", second.seq)
    expect(third).toMatchObject({ records: [], seq: second.seq })

    const all = [...first.records, ...second.records]
    const seqs = all.map((r) => r.seq)
    expect(seqs).toEqual([...seqs].sort((a, b) => a - b))
    expect(new Set(seqs).size).toBe(seqs.length)
    // Each record appears once, at its latest seq: 520 records, the last two being the edit and the tombstone.
    expect(all).toHaveLength(verses.length * 2)
    expect(second.seq).toBe(total)
    expect(all.at(-2)).toEqual({ type: "highlight", uid: `h-${verses[0]}`, seq: total - 1, createdAt: expect.any(Number), verse: verses[0], color: "blue" })
    expect(all.at(-1)).toMatchObject({ type: "bookmark", uid: `b-${verses[1]}`, seq: total, verse: verses[1], deletedAt: expect.any(Number) })
    for (const r of all) {
      const allowed = ["type", "uid", "seq", "deletedAt", "createdAt", "verse", r.type === "highlight" ? "color" : "label"]
      expect(Object.keys(r).filter((k) => !allowed.includes(k))).toEqual([])
    }
    expect(first.serverTime).toBeGreaterThan(0)
  })

  it("tells the time from a mutation, which writes nothing", async () => {
    const t = await makeConvex()
    const before = Date.now()
    const { serverTime } = await as(t, "user_ann").mutation(api.library.sync.now, {})
    expect(serverTime).toBeGreaterThanOrEqual(before)
    expect(serverTime).toBeLessThanOrEqual(Date.now())
    expect(await t.run((ctx) => ctx.db.query("userLibrary").collect())).toEqual([])
  })

  it("tells a cursor older than the purged tombstones to resync, but not a device starting from 0", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    for (let i = 0; i < 5; i++) await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16 + i, color: "green" })
    await t.run(async (ctx) => {
      const row = (await ctx.db.query("userLibrary").first())!
      await ctx.db.patch(row._id, { purgedThrough: 3 })
    })
    expect(await pull(t, "user_ann", 2)).toEqual({ resync: true })
    expect((await page(t, "user_ann", 3)).records.map((r) => r.seq)).toEqual([4, 5])
    expect((await page(t, "user_ann", 0)).records).toHaveLength(5)
  })

  it("never shows one user's records to another", async () => {
    const t = await makeConvex()
    await as(t, "user_ann").mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "yellow" })
    await as(t, "user_ann").mutation(api.library.bookmarks.set, { verse: ROMANS_8_28, label: "" })
    // Bob has no library yet, then one of his own.
    expect(await page(t, "user_bob", 0)).toMatchObject({ records: [], seq: 0 })
    await as(t, "user_bob").mutation(api.library.bookmarks.set, { verse: JOHN_3_16, label: "mine" })
    expect((await page(t, "user_bob", 0)).records).toEqual([{ type: "bookmark", uid: "b-42003016", seq: 1, createdAt: expect.any(Number), verse: JOHN_3_16, label: "mine" }])
    expect((await page(t, "user_ann", 0)).records.map((r) => r.type)).toEqual(["highlight", "bookmark"])
  })
})
