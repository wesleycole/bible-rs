import { createHash } from "node:crypto"
import { Schema } from "effect"
import { afterEach, describe, expect, it, vi } from "vitest"
import { Transcript } from "../shared/records.js"
import { parseReference } from "../convex/lib/references.js"
import { WEBHOOK_HEADER } from "../convex/lib/transcriber.js"
import { chunkNote, chunkTranscript } from "../convex/library/chunk.js"
import { PAGE_RECORDS } from "../convex/library/sync.js"
import { api, internal } from "../convex/_generated/api.js"
import { accountOf, SECRET } from "./harness.js"
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
    expect(await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "yellow" })).toEqual({
      result: "ok",
      seq: 1,
      // The record as stored, exactly as the feed sends it.
      record: (await page(t, "user_ann", 0)).records[0]
    })
    expect(await ann.mutation(api.library.highlights.set, { verse: ROMANS_8_28, color: "blue" })).toMatchObject({ result: "ok", seq: 2, record: { color: "blue" } })
    expect(await ann.query(api.library.highlights.list, {})).toEqual([
      { uid: "h-42003016", seq: 1, verse: JOHN_3_16, color: "yellow" },
      { uid: "h-44008028", seq: 2, verse: ROMANS_8_28, color: "blue" }
    ])
    expect(await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: null, baseSeq: 1 })).toMatchObject({ result: "ok", seq: 3, record: { deletedAt: expect.any(Number) } })
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
      record: { type: "highlight", uid: "h-42003016", seq: 2, verse: JOHN_3_16, color: "green", createdAt: expect.any(Number) }
    })
    expect(await ann.query(api.library.highlights.list, {})).toEqual([{ uid: "h-42003016", seq: 2, verse: JOHN_3_16, color: "green" }])
    // A write without baseSeq (a first write from a device with no copy) still wins.
    expect(await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "pink" })).toMatchObject({ result: "ok", seq: 3 })
  })

  it("never shows one user's records to another", async () => {
    const t = await makeConvex()
    await as(t, "user_ann").mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "yellow" })
    expect(await as(t, "user_bob").query(api.library.highlights.list, {})).toEqual([])
    expect(await as(t, "user_bob").mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "blue" })).toMatchObject({ result: "ok", seq: 1 })
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
    expect(await ann.mutation(api.library.bookmarks.set, { verse: JOHN_3_16, label: "Gospel" })).toMatchObject({ result: "ok", seq: 1 })
    expect(await ann.mutation(api.library.bookmarks.set, { verse: JOHN_3_16, label: "The gospel", baseSeq: 1 })).toMatchObject({ result: "ok", seq: 2 })
    expect(await ann.mutation(api.library.bookmarks.set, { verse: JOHN_3_16, label: null, baseSeq: 1 })).toMatchObject({
      result: "conflict",
      seq: 2,
      record: { type: "bookmark", label: "The gospel", createdAt: expect.any(Number) }
    })
    const rows = await t.run((ctx) => ctx.db.query("bookmarks").collect())
    expect(rows.map((r) => [r.verse, r.seq, r.label, r.deletedAt])).toEqual([[JOHN_3_16, 2, "The gospel", undefined]])
  })
})

type Page = Extract<Awaited<ReturnType<typeof pull>>, { records: unknown }>
const pull = (t: TestConvex, subject: string, after: number) => as(t, subject).query(api.library.sync.changes, { after })
const page = async (t: TestConvex, subject: string, after: number): Promise<Page> => {
  const p = await pull(t, subject, after)
  if (!("records" in p)) throw new Error(`expected a page, got ${JSON.stringify(p)}`)
  return p
}

describe("library.sync.changes", () => {
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
    // Every answer carries the record exactly as the feed sends it, so the writer holds what other devices get.
    const written = await ann.mutation(api.library.notes.upsert, first)
    expect(written).toEqual({ result: "ok", seq: 1, record: (await page(t, "user_ann", 0)).records[0] })
    // The answer was lost and the device sends it again, now against seq 0: the server already has it.
    expect(await ann.mutation(api.library.notes.upsert, { ...first, baseSeq: 0 })).toEqual(written)
    const rows = await t.run((ctx) => ctx.db.query("anchors").collect())
    expect(rows.map((r) => [r.chapter, r.from, r.to])).toEqual([[3, JOHN_3_36, JOHN_3_36], [4, 42_004_001, JOHN_4_2]])
    expect(await ann.mutation(api.library.notes.upsert, { ...first, text: "Edited on A", baseSeq: 1 })).toMatchObject({ result: "ok", seq: 2 })
    // B edited its copy from seq 1 while offline.
    // The answer carries the note that stood and the copy the edit became, both as the feed sends them.
    const conflict = await ann.mutation(api.library.notes.upsert, { ...first, text: "Edited on B", baseSeq: 1 })
    const feed = (await page(t, "user_ann", 1)).records
    expect(feed.map((r) => [r.uid, r.seq])).toEqual([["n1", 2], ["n1-c1", 3]])
    expect(conflict).toEqual({ result: "conflict", seq: 2, record: feed[0], copy: feed[1] })
    expect(Object.keys(feed[1]!)).not.toContain("userId")
    const items = await t.run((ctx) => ctx.db.query("items").collect())
    expect(items.map((i) => [i.uid, i.text, i.title, i.conflictOf])).toEqual([
      ["n1", "Edited on A", "Edited on A", undefined],
      ["n1-c1", "Edited on B", "Edited on B", "n1"]
    ])
    // A stale removal leaves the newer note alone.
    expect(await ann.mutation(api.library.notes.remove, { uid: "n1", baseSeq: 1 })).toMatchObject({ result: "conflict", seq: 2, record: { text: "Edited on A" } })
    const removed = await ann.mutation(api.library.notes.remove, { uid: "n1", baseSeq: 2 })
    expect(removed).toEqual({ result: "ok", seq: 4, record: (await page(t, "user_ann", 3)).records[0] })
    expect(removed).toMatchObject({ record: { uid: "n1", deletedAt: expect.any(Number), updatedAt: expect.any(Number) } })
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

describe("library.items.updateDetails", () => {
  it("sets a page's sermon and details, answers the stored record, refuses a missing sermon, and keeps a newer write", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    await ann.mutation(api.library.notes.upsert, { uid: "page", text: "Slide", anchors: [] })
    const userId = (await t.run((ctx) => ctx.db.query("users").withIndex("by_clerk", (q) => q.eq("clerkId", "user_ann")).unique()))!._id
    await t.run((ctx) =>
      ctx.db.insert("items", { userId, uid: "sermon", seq: 2, kind: "sermon", title: "", status: "ready", unreviewed: false, files: [], anchors: [], capturedAt: 0 })
    )
    expect((await errorOf(ann.mutation(api.library.items.updateDetails, { uid: "page", sermonUid: "nope", baseSeq: 1 }))).type).toBe("invalid_request")
    expect((await errorOf(ann.mutation(api.library.items.updateDetails, { uid: "page", sermonUid: "page", baseSeq: 1 }))).type).toBe("invalid_request")
    const details = { title: "Romans 8", speaker: "A. Preacher", startedAt: 1_000 }
    const set = await ann.mutation(api.library.items.updateDetails, { uid: "page", sermonUid: "sermon", offsetMs: 90_000, details, baseSeq: 1 })
    // Sequence numbers come from the user's counter, which the inserted sermon didn't move.
    expect(set).toEqual({ result: "ok", seq: 2, record: (await page(t, "user_ann", 1)).records.find((r) => r.uid === "page") })
    expect(set).toMatchObject({ record: { sermonUid: "sermon", offsetMs: 90_000, details, text: "Slide" } })
    // The same again writes nothing; a stale one keeps what stands.
    expect(await ann.mutation(api.library.items.updateDetails, { uid: "page", sermonUid: "sermon", offsetMs: 90_000, baseSeq: 2 })).toEqual(set)
    expect(await ann.mutation(api.library.items.updateDetails, { uid: "page", sermonUid: null, baseSeq: 1 })).toMatchObject({
      result: "conflict",
      seq: 2,
      record: { sermonUid: "sermon" }
    })
    const cleared = await ann.mutation(api.library.items.updateDetails, { uid: "page", sermonUid: null, offsetMs: null, details: null, baseSeq: 2 })
    expect(cleared).toMatchObject({ result: "ok", seq: 3 })
    expect(Object.keys((cleared as { record: object }).record)).not.toContain("sermonUid")
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
    expect(await ann.mutation(api.library.chats.save, { uid: "c1", title: "Grace", fileUid: "f1" })).toEqual({ result: "ok", seq: 1, record: (await page(t, "user_ann", 0)).records[0] })
    const library = async () => (await t.run((ctx) => ctx.db.query("userLibrary").first()))!
    expect((await library()).storageBytes).toBe(f1.bytes)
    const two = await upload(t, '{"messages":[1,2,3]}')
    const f2 = await ann.mutation(api.library.files.attach, { uid: "f2", ownerUid: "c1", role: "chat", ...two })
    expect(await ann.mutation(api.library.chats.save, { uid: "c1", title: "Grace", fileUid: "f2", baseSeq: 1 })).toMatchObject({ result: "ok", seq: 2, record: { fileUid: "f2" } })
    expect((await library()).storageBytes).toBe(f2.bytes)
    expect((await t.run((ctx) => ctx.db.query("files").collect())).map((f) => f.uid)).toEqual(["f2"])
    const fed = await page(t, "user_ann", 0)
    expect(fed).toMatchObject({ records: [{ type: "chat", uid: "c1", seq: 2, fileUid: "f2", url: f2.url, bytes: f2.bytes, title: "Grace" }] })
    expect(Object.keys(fed.records[0]!).sort()).toEqual(["bytes", "createdAt", "fileUid", "seq", "title", "type", "uid", "updatedAt", "url"])
    // A stale save keeps the server's chat, answers it, and frees the losing upload.
    const three = await upload(t, '{"messages":[9]}')
    await ann.mutation(api.library.files.attach, { uid: "f3", ownerUid: "c1", role: "chat", ...three })
    expect(await ann.mutation(api.library.chats.save, { uid: "c1", title: "Stale", fileUid: "f3", baseSeq: 1 })).toMatchObject({
      result: "conflict",
      seq: 2,
      record: { uid: "c1", fileUid: "f2", title: "Grace" }
    })
    expect((await library()).storageBytes).toBe(f2.bytes)
    expect(await ann.mutation(api.library.chats.remove, { uid: "c1", baseSeq: 2 })).toMatchObject({ result: "ok", seq: 3, record: { uid: "c1", deletedAt: expect.any(Number) } })
    expect((await library()).storageBytes).toBe(0)
  })
})

describe("purge, tombstones and storage", () => {
  const upload = (t: TestConvex, text: string) =>
    t.run(async (ctx) => {
      const id = await ctx.storage.store(new Blob([text], { type: "application/json" }))
      return { storageId: id as string, sha256: Buffer.from((await ctx.db.system.get(id))!.sha256, "base64").toString("hex") }
    })

  it("purges a user with 2,000 records in steps, files included, and leaves another user's", async () => {
    vi.useFakeTimers()
    try {
      const t = await makeConvex()
      const [ann, bob] = [as(t, "user_ann"), as(t, "user_bob")]
      await bob.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "pink" })
      await ann.mutation(api.library.files.attach, { uid: "f1", ownerUid: "c1", role: "chat", ...(await upload(t, "{}")) })
      await ann.mutation(api.library.chats.save, { uid: "c1", title: "Grace", fileUid: "f1" })
      await ann.mutation(api.library.notes.upsert, { uid: "n1", text: "x", anchors: [{ from: JOHN_3_16, to: JOHN_3_16, origin: "user" }] })
      const userId = (await t.run((ctx) => ctx.db.query("users").withIndex("by_clerk", (q) => q.eq("clerkId", "user_ann")).unique()))!._id
      for (let batch = 0; batch < 4; batch++) {
        await t.run(async (ctx) => {
          for (let i = 0; i < 500; i++) {
            const verse = 1_001 + batch * 500 + i
            await ctx.db.insert("bookmarks", { userId, uid: `b-${verse}`, seq: 10 + batch * 500 + i, verse, label: "" })
          }
        })
      }
      await t.mutation(api.account.purge, { secret: SECRET, clerkId: "user_ann" })
      await t.finishAllScheduledFunctions(vi.runAllTimers)
      const left = await t.run(async (ctx) => ({
        users: (await ctx.db.query("users").collect()).map((u) => u.clerkId),
        bookmarks: (await ctx.db.query("bookmarks").collect()).length,
        highlights: (await ctx.db.query("highlights").collect()).map((h) => h.verse),
        items: (await ctx.db.query("items").collect()).length,
        anchors: (await ctx.db.query("anchors").collect()).length,
        chunks: (await ctx.db.query("chunks").collect()).length,
        chats: (await ctx.db.query("chats").collect()).length,
        files: (await ctx.db.query("files").collect()).length,
        storage: (await ctx.db.system.query("_storage").collect()).length,
        libraries: (await ctx.db.query("userLibrary").collect()).length
      }))
      expect(left).toEqual({ users: ["user_bob"], bookmarks: 0, highlights: [JOHN_3_16], items: 0, anchors: 0, chunks: 0, chats: 0, files: 0, storage: 0, libraries: 1 })
    } finally {
      vi.useRealTimers()
    }
  })

  it("hard-deletes tombstones after 30 days and tells a cursor behind them to resync", async () => {
    vi.useFakeTimers({ toFake: ["Date"] })
    try {
      const t = await makeConvex()
      const ann = as(t, "user_ann")
      await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "pink" })
      await ann.mutation(api.library.highlights.set, { verse: ROMANS_8_28, color: "blue" })
      await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: null, baseSeq: 1 })
      vi.setSystemTime(Date.now() + 29 * 24 * 3600_000)
      expect(await t.mutation(internal.library.purge.tombstones, {})).toEqual({ deleted: 0 })
      vi.setSystemTime(Date.now() + 2 * 24 * 3600_000)
      expect(await t.mutation(internal.library.purge.tombstones, {})).toEqual({ deleted: 1 })
      expect((await t.run((ctx) => ctx.db.query("highlights").collect())).map((h) => h.verse)).toEqual([ROMANS_8_28])
      expect(await ann.query(api.library.sync.changes, { after: 2 })).toEqual({ resync: true })
      expect(await ann.query(api.library.sync.changes, { after: 3 })).toMatchObject({ records: [] })
    } finally {
      vi.useRealTimers()
    }
  })

  it("refuses a file past the plan's storage, attaching nothing, and /v1/me reports the storage", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    await t.run(async (ctx) => {
      const free = (await ctx.db.query("plans").withIndex("by_key", (q) => q.eq("key", "free")).unique())!
      await ctx.db.patch(free._id, { storageBytes: 10 })
    })
    const small = await ann.mutation(api.library.files.attach, { uid: "f1", ownerUid: "c1", role: "chat", ...(await upload(t, "12345")) })
    const big = await upload(t, "123456")
    expect((await errorOf(ann.mutation(api.library.files.attach, { uid: "f2", ownerUid: "c2", role: "chat", ...big }))).type).toBe("too_large")
    expect((await t.run((ctx) => ctx.db.query("files").collect())).map((f) => f.uid)).toEqual(["f1"])
    const me = await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_ann" })
    expect(me.library).toEqual({ storageUsed: small.bytes, storageLimit: 10 })

    // The refused upload is nobody's: the daily sweep deletes it once it is a day old, and keeps
    // the attached file and any blob a sermon's transcript holds.
    const transcript = await t.run(async (ctx) => {
      const id = await ctx.storage.store(new Blob(["{}"]))
      const user = (await ctx.db.query("users").first())!
      await ctx.db.insert("sermons", { userId: user._id, recordingId: "r", blobPath: "p", durationSec: 1, bytes: 45, trial: false, status: "done", transcript: id, createdAt: 0 })
      return id
    })
    const blobs = async () => (await t.run((ctx) => ctx.db.system.query("_storage").collect())).map((b) => b._id as string)
    expect(await t.mutation(internal.library.purge.orphans, {})).toEqual({ deleted: 0 })
    vi.useFakeTimers({ toFake: ["Date"] })
    try {
      vi.setSystemTime(Date.now() + 25 * 3600_000)
      expect(await t.mutation(internal.library.purge.orphans, {})).toEqual({ deleted: 1 })
    } finally {
      vi.useRealTimers()
    }
    const attached = (await t.run((ctx) => ctx.db.query("files").first()))!.storageId
    expect((await blobs()).sort()).toEqual([attached, transcript].sort())
    expect(await blobs()).not.toContain(big.storageId)
  })
})

describe("library.sermons (the transcription workflow)", () => {
  const SITE = "https://linger.convex.site"
  const HOUR = 3600_000
  const details = { title: "Grace", startedAt: 1_700_000_000_000 }
  const M4A = new Uint8Array([0, 0, 0, 24, 0x66, 0x74, 0x79, 0x70, 1, 2, 3, 4, 5, 6, 7, 8, 9])
  const ADTS = new Uint8Array([0xff, 0xf1, 0x50, 0x80, 0x01, 0x7f, 0xfc, 42])
  const hex = (bytes: Uint8Array) => createHash("sha256").update(bytes).digest("hex")
  const job = { status: "completed", text: "In the beginning God.", audio_duration: 61.2, language_code: "en", utterances: [{ start: 0, end: 61_200, speaker: "A", text: "In the beginning God." }] }

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.unstubAllEnvs()
    vi.useRealTimers()
  })

  /** What the Gateway's model says of every transcript: one reference on the grid, one that isn't. */
  const digest = { references: [{ ref: "Genesis 1:1", at: "00:00" }, { ref: "Nowhere 9:9" }], outline: "- Creation", summary: "God made everything." }
  /** The providers at `fetch`: AssemblyAI with one job, in whatever state `state` holds when it is asked, and the Gateway with `digest`. */
  const fakeProvider = (state: { job: object }) => {
    vi.stubEnv("ASSEMBLYAI_API_KEY", "key")
    vi.stubEnv("AI_GATEWAY_API_KEY", "key")
    vi.stubEnv("CONVEX_SITE_URL", SITE)
    const calls: Array<{ method: string; path: string; body?: any }> = []
    vi.stubGlobal("fetch", async (url: string, init: RequestInit) => {
      const path = new URL(url).pathname
      calls.push({ method: init.method ?? "GET", path, body: init.body ? JSON.parse(String(init.body)) : undefined })
      const json = (o: object) => new Response(JSON.stringify(o), { status: 200, headers: { "content-type": "application/json" } })
      if (path === "/v1/messages") return json({ content: [{ type: "tool_use", name: "digest", input: digest }], usage: { input_tokens: 1_000, output_tokens: 100 } })
      return init.method === "POST" ? json({ id: "job1" }) : init.method === "DELETE" ? json({}) : json({ id: "job1", ...state.job })
    })
    return calls
  }

  const upload = (t: TestConvex, bytes: Uint8Array<ArrayBuffer>, type: string) =>
    t.run(async (ctx) => {
      const id = await ctx.storage.store(new Blob([bytes], { type }))
      return { storageId: id as string, sha256: Buffer.from((await ctx.db.system.get(id))!.sha256, "base64").toString("hex") }
    })
  /** Uploads and attaches `bytes` in two slices (or parts) of the given role. */
  const attachSlices = async (t: TestConvex, user: string, uid: string, role: "uploadPart" | "audioPart", bytes: Uint8Array<ArrayBuffer>) => {
    const slices = [bytes.subarray(0, 5), bytes.subarray(5)]
    for (const [partIndex, slice] of slices.entries()) {
      await as(t, user).mutation(api.library.files.attach, { uid: `${uid}-${partIndex}`, ownerUid: uid, role, partIndex, ...(await upload(t, slice, "application/octet-stream")) })
    }
  }
  const itemOf = async (t: TestConvex, uid: string) => (await t.run((ctx) => ctx.db.query("items").filter((q) => q.eq(q.field("uid"), uid)).unique()))!
  const userOf = async (t: TestConvex, clerkId: string) => (await t.run((ctx) => ctx.db.query("users").withIndex("by_clerk", (q) => q.eq("clerkId", clerkId)).unique()))!
  /** The stored file: its bytes and type (convex-test keeps the Blob but not the `_storage` row's contentType, so the rows say octet-stream). */
  const storedOf = (t: TestConvex, uid: string) =>
    t.run(async (ctx) => {
      const file = (await ctx.db.query("files").filter((q) => q.eq(q.field("uid"), uid)).unique())!
      const blob = (await ctx.storage.get(file.storageId))!
      return { bytes: await blob.arrayBuffer(), type: blob.type }
    })
    .then(({ bytes, type }) => ({ bytes: new Uint8Array(bytes), type }))
  const hook = (t: TestConvex, secret: string) =>
    t.fetch("/assemblyai", { method: "POST", headers: { [WEBHOOK_HEADER]: secret, "content-type": "application/json" }, body: JSON.stringify({ transcript_id: "job1", status: "completed" }) })

  it("transcribes a Pro sermon: the slices join byte for byte, the transcript is its text, the real duration settles, the job and slices go", async () => {
    vi.useFakeTimers()
    const t = await makeConvex()
    await t.mutation(internal.admin.grant, { clerkId: "user_ann", days: 30 })
    const ann = as(t, "user_ann")
    const calls = fakeProvider({ job })
    expect(await ann.mutation(api.library.sermons.begin, { uid: "s1", details, recordedOn: "2026-10-18" })).toEqual({ uploadAudio: true, cloudTranscript: true })
    // Beginning again (a retry) makes nothing new.
    await ann.mutation(api.library.sermons.begin, { uid: "s1", details, recordedOn: "2026-10-18" })
    expect(await t.run((ctx) => ctx.db.query("sermons").collect())).toHaveLength(1)
    await attachSlices(t, "user_ann", "s1", "uploadPart", M4A)
    expect(await ann.mutation(api.library.sermons.finish, { uid: "s1", durationMs: 60_000, audioFileUid: hex(M4A) })).toMatchObject({
      result: "ok",
      record: { type: "item", uid: "s1", status: "uploading", details, files: [] }
    })
    // A second finish (the device's retry) answers the same and starts nothing new.
    expect(await ann.mutation(api.library.sermons.finish, { uid: "s1", durationMs: 60_000, audioFileUid: hex(M4A) })).toMatchObject({ result: "ok", record: { status: "uploading" } })
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    expect(await itemOf(t, "s1")).toMatchObject({ status: "processing", files: [{ role: "audio", uid: hex(M4A), bytes: M4A.length, durationMs: 60_000 }] })
    expect(await storedOf(t, hex(M4A))).toEqual({ bytes: M4A, type: "audio/mp4" })
    expect(calls).toHaveLength(1)
    expect(calls[0]).toMatchObject({
      method: "POST",
      path: "/v2/transcript",
      body: { audio_url: expect.stringContaining("/api/storage/"), speaker_labels: true, webhook_url: `${SITE}/assemblyai`, webhook_auth_header_name: WEBHOOK_HEADER }
    })
    const secret: string = calls[0]!.body.webhook_auth_header_value
    expect((await hook(t, "not-the-secret")).status).toBe(401)
    expect((await hook(t, secret)).status).toBe(200)
    // A second delivery is acknowledged and changes nothing.
    expect((await hook(t, secret)).status).toBe(200)
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    const ready = await itemOf(t, "s1")
    expect(ready).toMatchObject({ status: "ready", excerpt: "In the beginning God.", textFileUid: expect.any(String) })
    expect(ready.files.map((f) => [f.role, f.durationMs])).toEqual([["audio", 62_000], ["text", undefined]])
    const text = await storedOf(t, ready.textFileUid!)
    expect(text.type).toBe("application/json")
    // The file is what core reads: records::Transcript, by the generated Schema.
    expect(Schema.decodeUnknownSync(Transcript)(JSON.parse(Buffer.from(text.bytes).toString()))).toEqual({
      language: "en",
      durationMs: 61_200,
      segments: [{ startMs: 0, endMs: 61_200, speaker: "A", text: "In the beginning God." }]
    })
    expect((await t.run((ctx) => ctx.db.query("files").collect())).map((f) => f.role).sort()).toEqual(["audio", "text"])
    // Then the digest, once, through the Gateway with zero data retention and the transcript stamped a line a turn.
    expect(calls.map((c) => `${c.method} ${c.path}`)).toEqual(["POST /v2/transcript", "GET /v2/transcript/job1", "DELETE /v2/transcript/job1", "POST /v1/messages"])
    expect(calls[3]!.body).toMatchObject({
      model: "anthropic/claude-haiku-5.5",
      thinking: { type: "disabled" },
      tool_choice: { type: "tool", name: "digest" },
      messages: [{ role: "user", content: "Title: Grace\n\n[00:00] In the beginning God." }],
      providerOptions: { gateway: { zeroDataRetention: true } }
    })
    // The reference on the grid is a `model` anchor at its time; the one that isn't is dropped, not guessed.
    expect(ready).toMatchObject({ anchors: [{ from: 1_001, to: 1_001, origin: "model", atMs: 0 }], outline: "- Creation", summary: "God made everything." })
    expect((await t.run((ctx) => ctx.db.query("anchors").collect())).map((a) => [a.book, a.chapter, a.origin])).toEqual([[0, 1, "model"]])
    // The transcript is one searchable paragraph with its times.
    expect((await t.run((ctx) => ctx.db.query("chunks").collect())).map((c) => [c.ordinal, c.text, c.startMs, c.endMs, c.unreviewed])).toEqual([[0, "In the beginning God.", 0, 61_200, false]])
    const account = await accountOf(t, "user_ann")
    expect(account.reservations).toEqual([])
    expect(account.usage.map((u) => u.sermonSeconds)).toEqual([62])
    expect(account.ledger.map((l) => [l.kind, l.status, l.audioSeconds, l.inputTokens])).toEqual([["sermon", "ok", 62, undefined], ["digest", "ok", undefined, 1_000]])
    // The feed carries the item as the digest left it.
    expect((await page(t, "user_ann", 0)).records.at(-1)).toMatchObject({ type: "item", uid: "s1", status: "ready", textFileUid: ready.textFileUid, outline: "- Creation" })
    // What Ask's `read_item` reads: the window of paragraphs, stamped.
    expect(await ann.query(api.library.items.read, { uid: "s1", fromMs: 0, minutes: 2 })).toEqual({
      uid: "s1",
      kind: "sermon",
      title: "Grace",
      text: "[00:00] In the beginning God.",
      fromMs: 0,
      toMs: 61_200,
      truncated: false
    })
  })

  it("imports a sermon transcribed on the device: ready and searchable at once, digested after, spending nothing; a retry answers again, and a mismatched or malformed transcript is refused", async () => {
    vi.useFakeTimers()
    const t = await makeConvex()
    const bob = as(t, "user_bob")
    const calls = fakeProvider({ job })
    const transcript = { durationMs: 61_200, segments: [{ startMs: 0, endMs: 61_200, speaker: "A", text: "In the beginning God." }] }
    const text = JSON.stringify(transcript)
    const bytes = new TextEncoder().encode(text)
    const attachText = async (uid: string, b: Uint8Array<ArrayBuffer>) => {
      await bob.mutation(api.library.files.attach, { uid: hex(b), ownerUid: uid, role: "text", ...(await upload(t, b, "application/json")) })
      return hex(b)
    }
    const textFileUid = await attachText("old", bytes)
    const args = { uid: "old", details, recordedOn: "2026-06-07", durationMs: 61_200, textFileUid }
    // Bytes that aren't the file's, or a file that isn't a transcript, go nowhere.
    await expect(bob.mutation(api.library.sermons.import, { ...args, text: text + " " })).rejects.toThrow(/doesn't match/)
    const junk = new TextEncoder().encode(JSON.stringify({ hello: 1 }))
    const junkUid = await attachText("old", junk)
    await expect(bob.mutation(api.library.sermons.import, { ...args, textFileUid: junkUid, text: JSON.stringify({ hello: 1 }) })).rejects.toThrow(/isn't a transcript/)

    const answer = await bob.mutation(api.library.sermons.import, { ...args, text })
    expect(answer).toMatchObject({
      result: "ok",
      record: { type: "item", uid: "old", status: "ready", recordedOn: "2026-06-07", textFileUid, excerpt: "In the beginning God.", unreviewed: false }
    })
    // The text file is the item's; the junk one is only an attachment the device will never use.
    expect((await itemOf(t, "old")).files.filter((f) => f.role === "text").map((f) => f.uid)).toContain(textFileUid)
    // Searchable before the digest runs.
    expect((await t.run((ctx) => ctx.db.query("chunks").collect())).map((c) => c.text)).toEqual(["In the beginning God."])
    // A retry answers the same record and makes nothing new.
    expect(await bob.mutation(api.library.sermons.import, { ...args, text })).toMatchObject({ seq: answer.seq })
    expect(await t.run((ctx) => ctx.db.query("sermons").collect())).toHaveLength(1)
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    expect(calls.map((c) => `${c.method} ${c.path}`)).toEqual(["POST /v1/messages"])
    expect(await itemOf(t, "old")).toMatchObject({ status: "ready", outline: "- Creation", anchors: [{ origin: "model" }] })
    // Nothing spent: no reservation, no sermon usage, the trial still there.
    const account = await accountOf(t, "user_bob")
    expect(account.reservations).toEqual([])
    expect(account.usage.map((u) => u.sermonSeconds ?? 0).filter((n) => n > 0)).toEqual([])
    expect(account.ledger.map((l) => l.kind)).toEqual(["digest"])
    expect(await bob.mutation(api.library.sermons.begin, { uid: "new", details, recordedOn: "2026-10-18" })).toEqual({ uploadAudio: true, cloudTranscript: true })
  })

  it("spends a Free user's trial hour once: a failed join gives it back, a trial transcript keeps no audio, and then the plan allows nothing", async () => {
    vi.useFakeTimers()
    const t = await makeConvex()
    const bob = as(t, "user_bob")
    fakeProvider({ job })
    expect(await bob.mutation(api.library.sermons.begin, { uid: "s1", details, recordedOn: "2026-10-18" })).toEqual({ uploadAudio: true, cloudTranscript: true })
    await attachSlices(t, "user_bob", "s1", "uploadPart", M4A)
    // The device named a different file than it uploaded.
    await bob.mutation(api.library.sermons.finish, { uid: "s1", durationMs: 60_000, audioFileUid: hex(ADTS) })
    expect((await userOf(t, "user_bob")).trialHourUsed).toBe(true)
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    expect(await itemOf(t, "s1")).toMatchObject({ status: "error" })
    expect((await userOf(t, "user_bob")).trialHourUsed).toBe(false)
    expect((await accountOf(t, "user_bob")).reservations).toEqual([])

    expect(await bob.mutation(api.library.sermons.begin, { uid: "s2", details, recordedOn: "2026-10-19" })).toEqual({ uploadAudio: true, cloudTranscript: true })
    await attachSlices(t, "user_bob", "s2", "uploadPart", M4A)
    await bob.mutation(api.library.sermons.finish, { uid: "s2", durationMs: 60_000, audioFileUid: hex(M4A) })
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    const secret = (await t.run((ctx) => ctx.db.query("sermons").filter((q) => q.eq(q.field("status"), "transcribing")).unique()))!.webhookSecretHash
    expect(secret).toBeDefined()
    // The hash of the secret is all Convex keeps, so the webhook is sent from the workflow's side here.
    await t.mutation(internal.library.sermons.webhook, { assemblyId: "job1", secretHash: secret! })
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    const ready = await itemOf(t, "s2")
    expect(ready.status).toBe("ready")
    expect(ready.files.map((f) => f.role)).toEqual(["text"])
    expect((await accountOf(t, "user_bob")).ledger.map((l) => [l.kind, l.status])).toEqual([["sermon", "trial"], ["digest", "ok"]])

    expect(await bob.mutation(api.library.sermons.begin, { uid: "s3", details, recordedOn: "2026-10-20" })).toEqual({ uploadAudio: false, cloudTranscript: false })
    expect(await bob.mutation(api.library.sermons.finish, { uid: "s3", durationMs: 60_000 })).toMatchObject({ result: "ok", record: { status: "waiting" } })
    expect((await errorOf(bob.mutation(api.library.sermons.transcribeNow, { uid: "s3" }))).type).toBe("invalid_request")
  })

  it("checks on a job whose webhook is late from the cron, wakes it once done, and fails one stuck for three hours", async () => {
    vi.useFakeTimers()
    const t = await makeConvex()
    await t.mutation(internal.admin.grant, { clerkId: "user_ann", days: 30 })
    const ann = as(t, "user_ann")
    const state = { job: { status: "processing" } }
    fakeProvider(state)
    for (const uid of ["s1", "s2"]) {
      await ann.mutation(api.library.sermons.begin, { uid, details, recordedOn: "2026-10-18" })
      await attachSlices(t, "user_ann", uid, "uploadPart", M4A)
      await ann.mutation(api.library.sermons.finish, { uid, durationMs: 60_000, audioFileUid: hex(M4A) })
    }
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    expect((await itemOf(t, "s1")).status).toBe("processing")
    vi.setSystemTime(Date.now() + 31 * 60_000)
    expect(await t.action(internal.library.sermons.checkJobs, {})).toEqual({ failed: 0, woken: 0 })
    state.job = job
    expect(await t.action(internal.library.sermons.checkJobs, {})).toEqual({ failed: 0, woken: 2 })
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    expect((await itemOf(t, "s1")).status).toBe("ready")
    expect((await itemOf(t, "s2")).status).toBe("ready")

    await ann.mutation(api.library.sermons.begin, { uid: "s3", details, recordedOn: "2026-10-19" })
    await attachSlices(t, "user_ann", "s3", "uploadPart", M4A)
    state.job = { status: "processing" }
    await ann.mutation(api.library.sermons.finish, { uid: "s3", durationMs: 60_000, audioFileUid: hex(M4A) })
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    const before = await accountOf(t, "user_ann")
    expect(before.reservations.map((r) => r.seconds)).toEqual([60])
    vi.setSystemTime(Date.now() + 3 * HOUR + 60_000)
    expect(await t.action(internal.library.sermons.checkJobs, {})).toEqual({ failed: 1, woken: 0 })
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    expect(await itemOf(t, "s3")).toMatchObject({ status: "error" })
    const after = await accountOf(t, "user_ann")
    expect(after.reservations).toEqual([])
    expect(after.usage.map((u) => u.sermonSeconds)).toEqual(before.usage.map((u) => u.sermonSeconds))
  })

  it("finishes a recording whose device died with the parts it has, as an AAC stream the sermon waits on until \"Transcribe now\"", async () => {
    vi.useFakeTimers()
    const t = await makeConvex()
    await t.mutation(internal.admin.grant, { clerkId: "user_ann", days: 30 })
    const ann = as(t, "user_ann")
    fakeProvider({ job })
    await ann.mutation(api.library.sermons.begin, { uid: "dead", details, recordedOn: "2026-10-18" })
    await attachSlices(t, "user_ann", "dead", "audioPart", ADTS)
    await ann.mutation(api.library.sermons.begin, { uid: "alive", details, recordedOn: "2026-10-18" })
    vi.setSystemTime(Date.now() + 2 * HOUR - 60_000)
    // Still recording: its newest part is under two hours old.
    await attachSlices(t, "user_ann", "alive", "audioPart", ADTS)
    vi.setSystemTime(Date.now() + 2 * 60_000)
    expect(await t.mutation(internal.library.sermons.finishDead, {})).toEqual({ finished: 1 })
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    expect((await itemOf(t, "alive")).status).toBe("recording")
    const dead = await itemOf(t, "dead")
    expect(dead.status).toBe("waiting")
    expect(dead.files.map((f) => [f.role, f.bytes])).toEqual([["audio", ADTS.length]])
    expect(await storedOf(t, hex(ADTS))).toEqual({ bytes: ADTS, type: "audio/aac" })
    expect(await ann.mutation(api.library.sermons.transcribeNow, { uid: "dead" })).toMatchObject({ result: "ok", record: { status: "uploading" } })
    await t.finishAllScheduledFunctions(vi.runAllTimers)
    expect((await itemOf(t, "dead")).status).toBe("processing")
    expect((await errorOf(ann.mutation(api.library.sermons.transcribeNow, { uid: "dead" }))).type).toBe("invalid_request")
  })
})

describe("library.search, items.read, items.forChapter and export (M2's Convex half)", () => {
  it("reads a one-chapter book's single number as verses", () => {
    expect(parseReference("Jude 3")).toEqual({ from: 64_001_003, to: 64_001_003 })
    expect(parseReference("2 John 7-9")).toEqual({ from: 62_001_007, to: 62_001_009 })
    expect(parseReference("Romans 8")).toEqual({ from: 44_008_001, to: 44_008_039 })
  })

  const JOHN_3_36 = 42_003_036
  const JOHN_4_2 = 42_004_002
  const sentence = (n: number, word: string) => Array.from({ length: n }, (_, i) => `${word}${i}`).join(" ") + "."

  it("chunks a transcript into paragraphs: a turn change or a pause closes one once it has MIN words, TARGET words closes it anywhere, never inside a sentence", () => {
    const transcript = {
      language: "en",
      durationMs: 400_000,
      segments: [
        // Under MIN words: the speaker change after it doesn't close the paragraph.
        { startMs: 0, endMs: 10_000, speaker: "A", text: sentence(20, "a") },
        { startMs: 10_000, endMs: 40_000, speaker: "B", text: sentence(40, "b") },
        // Now past MIN: a pause of PAUSE closes it.
        { startMs: 41_500, endMs: 50_000, speaker: "B", text: sentence(10, "c") },
        // One long turn is split by sentence once it is past TARGET, its times shared by position.
        { startMs: 100_000, endMs: 200_000, speaker: "A", text: `${sentence(200, "d")} ${sentence(200, "e")}` }
      ]
    }
    const chunks = chunkTranscript(transcript, true)
    expect(chunks.map((c) => [c.ordinal, c.startMs, c.endMs, c.text.split(/\s+/).length, c.unreviewed])).toEqual([
      [0, 0, 40_000, 60, true],
      // The ten words of "c" are under MIN, so the speaker change after them closes nothing.
      [1, 41_500, 150_000, 210, true],
      [2, 150_000, 200_000, 200, true]
    ])
    expect(chunks[0]!.text.startsWith("a0 a1")).toBe(true)
    expect(chunks[1]!.text.startsWith("c0 c1")).toBe(true)
    expect(chunks[2]!.text.startsWith("e0 e1")).toBe(true)
    expect(chunkNote("  A thought.  ")).toEqual([{ ordinal: 0, text: "A thought.", focus: false, unreviewed: false }])
    expect(chunkNote(" ")).toEqual([])
  })

  it("searches one person's library only, puts the chunk anchored to the passage first, and filters by kind", async () => {
    const t = await makeConvex()
    const [ann, bob] = [as(t, "user_ann"), as(t, "user_bob")]
    await ann.mutation(api.library.notes.upsert, { uid: "n1", text: "Grace abounds in the morning.", anchors: [], capturedAt: 2_000 })
    await ann.mutation(api.library.notes.upsert, { uid: "n2", text: "Grace for the whole world.", anchors: [{ from: JOHN_3_16, to: JOHN_3_16, origin: "user" }], capturedAt: 1_000 })
    await bob.mutation(api.library.notes.upsert, { uid: "n1", text: "Grace, grace, grace.", anchors: [{ from: JOHN_3_16, to: JOHN_3_16, origin: "user" }] })
    const hits = await ann.action(api.library.search.search, { query: "grace", passage: { from: JOHN_3_16, to: JOHN_3_16 } })
    expect(hits.map((h) => [h.itemUid, h.anchored, h.text])).toEqual([
      ["n2", true, "Grace for the whole world."],
      ["n1", false, "Grace abounds in the morning."]
    ])
    expect(hits[0]).toEqual({ itemUid: "n2", kind: "note", title: "Grace for the whole world.", date: 1_000, text: "Grace for the whole world.", anchored: true, unreviewed: false })
    // The passage alone, without words, still finds what is anchored to it.
    expect((await ann.action(api.library.search.search, { query: "", passage: { from: JOHN_3_16, to: JOHN_3_16 } })).map((h) => h.itemUid)).toEqual(["n2"])
    // Nothing of Bob's; a kind he has nothing of answers nothing.
    expect((await bob.action(api.library.search.search, { query: "grace" })).map((h) => h.itemUid)).toEqual(["n1"])
    expect(await bob.action(api.library.search.search, { query: "grace", kinds: ["sermon"] })).toEqual([])
    // Someone with no library yet.
    expect(await as(t, "user_new").action(api.library.search.search, { query: "grace" })).toEqual([])
    // Editing the note rebuilds its chunk; removing it leaves none.
    await ann.mutation(api.library.notes.upsert, { uid: "n1", text: "Mercy abounds.", anchors: [], baseSeq: 1 })
    expect((await ann.action(api.library.search.search, { query: "grace" })).map((h) => h.itemUid)).toEqual(["n2"])
    await ann.mutation(api.library.notes.remove, { uid: "n2", baseSeq: 2 })
    expect(await ann.action(api.library.search.search, { query: "grace" })).toEqual([])
    expect((await t.run((ctx) => ctx.db.query("chunks").collect())).map((c) => c.text).sort()).toEqual(["Grace, grace, grace.", "Mercy abounds."])
  })

  it("reads a note whole, and lists a chapter's items with a range that begins in the chapter before", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    await ann.mutation(api.library.notes.upsert, { uid: "n1", text: "Jesus baptizes more than John.", anchors: [{ from: JOHN_3_36, to: JOHN_4_2, origin: "user" }] })
    await ann.mutation(api.library.notes.upsert, { uid: "n2", text: "Living water.", anchors: [{ from: 42_004_010, to: 42_004_014, origin: "user" }] })
    await ann.mutation(api.library.notes.upsert, { uid: "gone", text: "Deleted.", anchors: [{ from: 42_004_001, to: 42_004_001, origin: "user" }] })
    await ann.mutation(api.library.notes.remove, { uid: "gone", baseSeq: 3 })
    expect(await ann.query(api.library.items.read, { uid: "n1" })).toEqual({ uid: "n1", kind: "note", title: "Jesus baptizes more than John.", text: "Jesus baptizes more than John.", truncated: false })
    expect(await ann.query(api.library.items.read, { uid: "nope" })).toBeNull()
    expect(await ann.query(api.library.items.forChapter, { book: 42, chapter: 4 })).toEqual([
      { uid: "n1", kind: "note", title: "Jesus baptizes more than John.", from: JOHN_3_36, to: JOHN_4_2, origin: "user" },
      { uid: "n2", kind: "note", title: "Living water.", from: 42_004_010, to: 42_004_014, origin: "user" }
    ])
    expect((await ann.query(api.library.items.forChapter, { book: 42, chapter: 3 })).map((i) => i.uid)).toEqual(["n1"])
    expect(await as(t, "user_bob").query(api.library.items.forChapter, { book: 42, chapter: 4 })).toEqual([])
  })

  it("exports the library as a zip anyone can open, kept as the person's file for a day", async () => {
    vi.useFakeTimers()
    try {
      const t = await makeConvex()
      const ann = as(t, "user_ann")
      await ann.mutation(api.library.notes.upsert, { uid: "n1", text: "Grace for the whole world.", anchors: [{ from: JOHN_3_16, to: JOHN_3_16, origin: "user" }], capturedAt: Date.UTC(2026, 9, 18) })
      const started = await ann.action(api.library.export.start, {})
      expect(started).toEqual({ url: expect.stringContaining("/api/storage/"), expiresAt: Date.now() + 24 * 3600_000 })
      const file = (await t.run((ctx) => ctx.db.query("files").unique()))!
      expect(file.role).toBe("export")
      expect(file.ownerUid).toBeUndefined()
      const bytes = new Uint8Array(await t.run(async (ctx) => (await ctx.storage.get(file.storageId))!.arrayBuffer()))
      const text = Buffer.from(bytes).toString("latin1")
      // A zip: local headers, then the central directory and its end record.
      expect(Array.from(bytes.subarray(0, 4))).toEqual([0x50, 0x4b, 0x03, 0x04])
      expect(text).toContain("PK\x01\x02")
      expect(text.slice(-22, -18)).toBe("PK\x05\x06")
      expect(text).toContain("README.md")
      expect(text).toContain("2026-10-18-grace-for-the-whole-world-n1/grace-for-the-whole-world.md")
      expect(text).toContain("- References:\n  - John 3:16\n")
      // A day on, the file is gone.
      await t.finishAllScheduledFunctions(vi.runAllTimers)
      expect(await t.run((ctx) => ctx.db.query("files").collect())).toEqual([])
      expect(await t.run((ctx) => ctx.db.system.query("_storage").collect())).toEqual([])
      expect(await as(t, "user_new").action(api.library.export.start, {})).toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })
})
