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

describe("library.notes", () => {
  const JOHN_3_36 = 42_003_036
  const JOHN_4_2 = 42_004_002

  it("writes a note with its anchors split per chapter, answers a retry without a new seq, and keeps a stale edit as a conflict copy", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    const anchors = [{ from: JOHN_3_36, to: JOHN_4_2, origin: "user" as const }]
    const first = { uid: "n1", text: "Belief\nand life", anchors }
    expect(await ann.mutation(api.library.notes.upsert, first)).toEqual({ result: "ok", seq: 1 })
    // The answer was lost and the device sends it again, now against seq 0: the server already has it.
    expect(await ann.mutation(api.library.notes.upsert, { ...first, baseSeq: 0 })).toEqual({ result: "ok", seq: 1 })
    const rows = await t.run((ctx) => ctx.db.query("anchors").collect())
    expect(rows.map((r) => [r.chapter, r.from, r.to])).toEqual([[3, JOHN_3_36, JOHN_3_36], [4, 42_004_001, JOHN_4_2]])
    expect(await ann.mutation(api.library.notes.upsert, { ...first, text: "Edited on A", baseSeq: 1 })).toEqual({ result: "ok", seq: 2 })
    // B edited its copy from seq 1 while offline.
    // The answer carries the note that stood, in the feed's shape, and where the edit went.
    const conflict = await ann.mutation(api.library.notes.upsert, { ...first, text: "Edited on B", baseSeq: 1 })
    expect(conflict).toMatchObject({ result: "conflict", seq: 2, copyUid: "n1-c1", copySeq: 3, record: { uid: "n1", seq: 2, text: "Edited on A" } })
    expect(Object.keys((conflict as { record: object }).record)).not.toContain("userId")
    const items = await t.run((ctx) => ctx.db.query("items").collect())
    expect(items.map((i) => [i.uid, i.text, i.title, i.conflictOf])).toEqual([
      ["n1", "Edited on A", "Edited on A", undefined],
      ["n1-c1", "Edited on B", "Edited on B", "n1"]
    ])
    // A stale removal leaves the newer note alone.
    expect(await ann.mutation(api.library.notes.remove, { uid: "n1", baseSeq: 1 })).toMatchObject({ result: "conflict", seq: 2, record: { text: "Edited on A" } })
    expect(await ann.mutation(api.library.notes.remove, { uid: "n1", baseSeq: 2 })).toMatchObject({ result: "ok" })
    expect(await t.run((ctx) => ctx.db.query("anchors").withIndex("by_item", (q) => q.eq("itemId", items[0]!._id)).collect())).toEqual([])
  })

  it("refuses an anchor off the grid or backwards", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    for (const anchors of [[{ from: 42_022_001, to: 42_022_001, origin: "user" as const }], [{ from: JOHN_4_2, to: JOHN_3_36, origin: "typed" as const }]]) {
      expect((await errorOf(ann.mutation(api.library.notes.upsert, { uid: "n", text: "x", anchors }))).type).toBe("invalid_request")
    }
    expect(await t.run((ctx) => ctx.db.query("items").collect())).toEqual([])
  })
})

describe("library.chats and files", () => {
  const upload = (t: TestConvex, text: string) =>
    t.run(async (ctx) => {
      const id = await ctx.storage.store(new Blob([text], { type: "application/json" }))
      // Convex keeps it as base64 (as convex-test does); the client sends hex.
      return { storageId: id as string, sha256: Buffer.from((await ctx.db.system.get(id))!.sha256, "base64").toString("hex") }
    })

  it("saves a chat over its attached file, counts storage, frees a replaced file, and feeds no _ids", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    const one = await upload(t, '{"messages":[1]}')
    expect((await errorOf(ann.mutation(api.library.chats.save, { uid: "c1", title: "Grace", fileUid: "f1" }))).type).toBe("invalid_request")
    const wrongSum = await errorOf(ann.mutation(api.library.files.attach, { uid: "f1", ownerUid: "c1", role: "chat", ...one, sha256: "00" }))
    expect(wrongSum.type).toBe("invalid_request")
    const f1 = await ann.mutation(api.library.files.attach, { uid: "f1", ownerUid: "c1", role: "chat", ...one })
    expect(await ann.mutation(api.library.files.attach, { uid: "f1", ownerUid: "c1", role: "chat", ...one })).toEqual(f1)
    expect(await ann.mutation(api.library.chats.save, { uid: "c1", title: "Grace", fileUid: "f1" })).toEqual({ result: "ok", seq: 1 })
    const library = async () => (await t.run((ctx) => ctx.db.query("userLibrary").first()))!
    expect((await library()).storageBytes).toBe(f1.bytes)
    const two = await upload(t, '{"messages":[1,2,3]}')
    const f2 = await ann.mutation(api.library.files.attach, { uid: "f2", ownerUid: "c1", role: "chat", ...two })
    expect(await ann.mutation(api.library.chats.save, { uid: "c1", title: "Grace", fileUid: "f2", baseSeq: 1 })).toEqual({ result: "ok", seq: 2 })
    expect((await library()).storageBytes).toBe(f2.bytes)
    expect((await t.run((ctx) => ctx.db.query("files").collect())).map((f) => f.uid)).toEqual(["f2"])
    const page = await ann.query(api.library.sync.changes, { after: 0 })
    expect(page).toMatchObject({ records: [{ type: "chat", uid: "c1", seq: 2, fileUid: "f2", url: f2.url, bytes: f2.bytes, title: "Grace" }] })
    if (!("records" in page)) throw new Error("a page")
    expect(Object.keys(page.records[0]!).sort()).toEqual(["bytes", "createdAt", "fileUid", "seq", "title", "type", "uid", "url"])
    // A stale save keeps the server's chat, answers it, and frees the losing upload.
    const three = await upload(t, '{"messages":[9]}')
    await ann.mutation(api.library.files.attach, { uid: "f3", ownerUid: "c1", role: "chat", ...three })
    expect(await ann.mutation(api.library.chats.save, { uid: "c1", title: "Stale", fileUid: "f3", baseSeq: 1 })).toMatchObject({
      result: "conflict",
      seq: 2,
      record: { uid: "c1", fileUid: "f2", title: "Grace" }
    })
    expect((await library()).storageBytes).toBe(f2.bytes)
    expect(await ann.mutation(api.library.chats.remove, { uid: "c1", baseSeq: 2 })).toEqual({ result: "ok", seq: 3 })
    expect((await library()).storageBytes).toBe(0)
  })
})
