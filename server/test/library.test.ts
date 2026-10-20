import { describe, expect, it } from "vitest"
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

  it("keeps the server's copy when the writer's baseSeq is stale", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "yellow" }) // seq 1
    await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "green", baseSeq: 1 }) // seq 2, from device A
    // Device B edited from the copy at seq 1 while offline.
    expect(await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "pink", baseSeq: 1 })).toEqual({
      result: "conflict",
      seq: 2,
      color: "green"
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

  it("spike: the Effect handler costs about what the plain one does", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    const time = async (run: (i: number) => Promise<unknown>) => {
      const samples: Array<number> = []
      for (let i = 0; i < 150; i++) {
        const start = performance.now()
        await run(i)
        samples.push(performance.now() - start)
      }
      samples.sort((a, b) => a - b)
      return { median: samples[75]!, p95: samples[142]! }
    }
    // Warm both paths, then alternate so neither gets the colder JIT.
    await ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: "yellow" })
    await ann.mutation(api.library.spike.setPlain, { verse: JOHN_3_16, color: "yellow" })
    const effect = await time((i) => ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: i % 2 ? "green" : "blue" }))
    const plain = await time((i) => ann.mutation(api.library.spike.setPlain, { verse: JOHN_3_16, color: i % 2 ? "green" : "blue" }))
    const effect2 = await time((i) => ann.mutation(api.library.highlights.set, { verse: JOHN_3_16, color: i % 2 ? "green" : "blue" }))
    console.log(
      `highlights.set in convex-test: Effect ${effect.median.toFixed(2)} ms median (p95 ${effect.p95.toFixed(2)}), ` +
        `plain ${plain.median.toFixed(2)} ms (p95 ${plain.p95.toFixed(2)}), Effect again ${effect2.median.toFixed(2)} ms`
    )
    expect(Math.min(effect.median, effect2.median)).toBeLessThan(plain.median + 2)
  })
})
