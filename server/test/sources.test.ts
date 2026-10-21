import { describe, expect, it } from "vitest"
import { api, makeConvex, SECRET, type TestConvex } from "./harness.js"

const GEN = (chapter: number, verse: number) => chapter * 1_000 + verse

const as = (t: TestConvex, subject: string) =>
  t.withIdentity({ subject, issuer: "https://clerk.test", tokenIdentifier: `https://clerk.test|${subject}` })

const mhc = { key: "mhc", title: "Commentary on the Whole Bible", author: "Matthew Henry", year: 1710, tradition: "Reformed", kind: "commentary", licence: "Public domain" } as const
const wcf = { key: "westminster", title: "Westminster Standards", author: "Westminster Assembly", year: 1647, tradition: "Reformed", kind: "confession", licence: "Public domain" } as const

describe("sources", () => {
  it("finds the entries on a passage, including a range that starts in an earlier chapter, once their load is committed", async () => {
    const t = await makeConvex()
    const ann = as(t, "user_ann")
    await ann.mutation(api.library.highlights.set, { verse: GEN(1, 1), color: "yellow" })
    const passage = (from: number, to: number, works?: Array<string>) => ann.query(api.sources.passage, works ? { from, to, works } : { from, to })

    await t.mutation(api.sources.load, {
      secret: SECRET,
      work: "mhc",
      version: "v1",
      kind: "commentary",
      entries: [
        { key: "Gen.1.1-2.3", from: GEN(1, 1), to: GEN(2, 3), heading: "The creation", text: "Six days." },
        { key: "Gen.2.4-2.25", from: GEN(2, 4), to: GEN(2, 25), text: "The garden." }
      ]
    })
    await t.mutation(api.sources.load, {
      secret: SECRET,
      work: "westminster",
      version: "v1",
      kind: "confession",
      entries: [
        { key: "WCF.4.2", heading: "Chapter 4: Of Creation", text: "After God had made all other creatures, he created man.", cites: [{ from: GEN(1, 27), to: GEN(1, 27) }, { from: GEN(2, 2), to: GEN(2, 2) }] },
        { key: "WCF.21.7", text: "One day in seven.", cites: [{ from: GEN(2, 3), to: GEN(2, 3) }] }
      ]
    })
    // Loaded but not committed: readers see nothing yet.
    expect(await passage(GEN(2, 1), GEN(2, 2))).toEqual([])

    await t.mutation(api.sources.commit, { secret: SECRET, ...mhc, version: "v1" })
    await t.mutation(api.sources.commit, { secret: SECRET, ...wcf, version: "v1" })
    expect(await passage(GEN(2, 1), GEN(2, 2))).toEqual([
      { work: "westminster", key: "WCF.4.2", heading: "Chapter 4: Of Creation", text: "After God had made all other creatures, he created man.", cites: [{ from: GEN(1, 27), to: GEN(1, 27) }, { from: GEN(2, 2), to: GEN(2, 2) }] },
      { work: "mhc", key: "Gen.1.1-2.3", from: GEN(1, 1), to: GEN(2, 3), heading: "The creation", text: "Six days." }
    ])
    expect((await passage(GEN(2, 3), GEN(2, 4))).map((e) => e.key)).toEqual(["WCF.21.7", "Gen.1.1-2.3", "Gen.2.4-2.25"])
    expect((await passage(GEN(2, 3), GEN(2, 4), ["mhc"])).map((e) => e.key)).toEqual(["Gen.1.1-2.3", "Gen.2.4-2.25"])
    expect(await passage(GEN(3, 1), GEN(3, 1))).toEqual([])

    // A reload: the new version is invisible until committed, then the old one is purged in steps.
    await t.mutation(api.sources.load, {
      secret: SECRET,
      work: "mhc",
      version: "v2",
      kind: "commentary",
      entries: [{ key: "Gen.2.1-2.3", from: GEN(2, 1), to: GEN(2, 3), text: "The sabbath." }]
    })
    expect((await passage(GEN(2, 1), GEN(2, 1), ["mhc"])).map((e) => e.key)).toEqual(["Gen.1.1-2.3"])
    await t.mutation(api.sources.commit, { secret: SECRET, ...mhc, version: "v2" })
    expect((await passage(GEN(2, 1), GEN(2, 1), ["mhc"])).map((e) => e.key)).toEqual(["Gen.2.1-2.3"])
    await expect(t.mutation(api.sources.purge, { secret: SECRET, work: "mhc", version: "v2" })).rejects.toThrow()
    expect(await t.mutation(api.sources.purge, { secret: SECRET, work: "mhc", version: "v1" })).toEqual({ more: false })
    const left = await t.run(async (ctx) => ({
      entries: (await ctx.db.query("sourceEntries").collect()).map((e) => `${e.work}/${e.version}/${e.key}`),
      anchors: (await ctx.db.query("sourceAnchors").collect()).map((a) => `${a.work}/${a.version}/${a.chapter}`)
    }))
    expect(left).toEqual({
      entries: ["westminster/v1/WCF.4.2", "westminster/v1/WCF.21.7", "mhc/v2/Gen.2.1-2.3"],
      anchors: ["westminster/v1/1", "westminster/v1/2", "westminster/v1/2", "mhc/v2/2"]
    })
  })
})
