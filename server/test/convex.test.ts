import { describe, expect, it, vi } from "vitest"
import { api, internal } from "../convex/_generated/api.js"
import { makeConvex, SECRET } from "./harness.js"

const begin = (clerkId: string, questionId = crypto.randomUUID()) => ({
  secret: SECRET,
  clerkId,
  questionId,
  round: 0,
  mode: "standard" as const,
  estimatedInputTokens: 2000
})

describe("usage.beginRound", () => {
  it("never admits more questions than the allowance under concurrent calls at the boundary", async () => {
    vi.useFakeTimers({ toFake: ["Date"] })
    try {
      vi.setSystemTime(Date.UTC(2026, 9, 5, 12))
      const t = await makeConvex()
      // Put the user one question short of the monthly 30, early in a day.
      await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_edge" })
      await t.run(async (ctx) => {
        const user = (await ctx.db.query("users").first())!
        await ctx.db.insert("usage", {
          userId: user._id,
          period: "2026-10",
          questions: 29,
          deepQuestions: 0,
          costMicros: 0,
          sermonSeconds: 0,
          day: "2026-10-05",
          questionsToday: 0
        })
      })
      const results = await Promise.all(Array.from({ length: 8 }, () => t.mutation(api.usage.beginRound, begin("user_edge"))))
      expect(results.filter((r) => r.ok)).toHaveLength(1)
      expect(results.filter((r) => !r.ok).map((r) => !r.ok && r.error.type)).toEqual(Array(7).fill("allowance_exhausted"))
      const usage = await t.run((ctx) => ctx.db.query("usage").collect())
      expect(usage[0].questions).toBe(30)
    } finally {
      vi.useRealTimers()
    }
  })

  it("refuses callers without the server secret", async () => {
    const t = await makeConvex()
    await expect(t.mutation(api.usage.beginRound, { ...begin("user_x"), secret: "wrong" })).rejects.toThrow(/unauthorized/)
    await expect(t.query(api.users.billingProfile, { secret: "", clerkId: "user_x" })).rejects.toThrow(/unauthorized/)
  })

  it("treats a repeated round 0 as a retry of the same question, not a new one", async () => {
    const t = await makeConvex()
    const id = crypto.randomUUID()
    const a = await t.mutation(api.usage.beginRound, begin("user_retry", id))
    const b = await t.mutation(api.usage.beginRound, begin("user_retry", id))
    expect(a.ok && b.ok).toBe(true)
    if (a.ok && b.ok) expect(b.questionsLeft).toBe(a.questionsLeft)
  })

  it("rate-limits a burst of new questions to 10 a minute", async () => {
    const t = await makeConvex()
    await t.mutation(internal.admin.grant, { clerkId: "user_burst", days: 1 })
    const results = []
    for (let i = 0; i < 12; i++) results.push(await t.mutation(api.usage.beginRound, begin("user_burst")))
    const denied = results.filter((r) => !r.ok)
    expect(denied).toHaveLength(2)
    expect(!denied[0].ok && denied[0].error.type).toBe("rate_limited")
    expect(!denied[0].ok && denied[0].error.retryAfter).toBeGreaterThan(0)
  })
})

describe("usage.settleRound and the sweeper", () => {
  it("settles once: a second settle of the same reservation is a no-op", async () => {
    const t = await makeConvex()
    const r = await t.mutation(api.usage.beginRound, begin("user_settle"))
    if (!r.ok) throw new Error("denied")
    const usage = { inputTokens: 100, outputTokens: 10, cacheReadTokens: 0, cacheWriteTokens: 0 }
    const first = await t.mutation(api.usage.settleRound, { secret: SECRET, reservationId: r.reservationId, usage, status: "ok", durationMs: 5 })
    const second = await t.mutation(api.usage.settleRound, { secret: SECRET, reservationId: r.reservationId, usage, status: "ok", durationMs: 5 })
    expect(first.settled).toBe(true)
    expect(second.settled).toBe(false)
    const ledger = await t.run((ctx) => ctx.db.query("ledger").collect())
    expect(ledger).toHaveLength(1)
  })

  it("charges reservations older than 15 minutes in full", async () => {
    vi.useFakeTimers({ toFake: ["Date"] })
    try {
      vi.setSystemTime(Date.UTC(2026, 9, 5, 12))
      const t = await makeConvex()
      const r = await t.mutation(api.usage.beginRound, begin("user_crash"))
      if (!r.ok) throw new Error("denied")
      const reserved = (await t.run((ctx) => ctx.db.get(r.reservationId)))!.amountMicros!
      vi.setSystemTime(Date.UTC(2026, 9, 5, 12, 10))
      expect((await t.mutation(internal.usage.sweepReservations, {})).swept).toBe(0)
      vi.setSystemTime(Date.UTC(2026, 9, 5, 12, 16))
      expect((await t.mutation(internal.usage.sweepReservations, {})).swept).toBe(1)
      const ledger = await t.run((ctx) => ctx.db.query("ledger").collect())
      expect(ledger[0]).toMatchObject({ status: "swept", costMicros: reserved })
    } finally {
      vi.useRealTimers()
    }
  })
})

describe("entitlements.upsert", () => {
  const row = {
    secret: SECRET,
    source: "app_store" as const,
    productId: "pro.monthly",
    status: "active" as const,
    periodStart: Date.UTC(2026, 9, 1),
    expiresAt: Date.UTC(2099, 0, 1),
    originalId: "2000000123",
    environment: "Sandbox"
  }

  it("is idempotent per original id, and later notifications find the owner through it", async () => {
    const t = await makeConvex()
    const me = await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_buyer" })
    const first = await t.mutation(api.entitlements.upsert, { ...row, owner: { appAccountToken: me.user.appAccountToken } })
    const again = await t.mutation(api.entitlements.upsert, { ...row, owner: { clerkId: "user_buyer" } })
    // A renewal notification without the account token still lands on the same row.
    const renewal = await t.mutation(api.entitlements.upsert, { ...row, expiresAt: Date.UTC(2099, 1, 1) })
    expect(first.ok && first.created).toBe(true)
    expect(again.ok && !again.created).toBe(true)
    expect(renewal.ok && !renewal.created).toBe(true)
    const rows = await t.run((ctx) => ctx.db.query("entitlements").collect())
    expect(rows).toHaveLength(1)
    expect(rows[0].expiresAt).toBe(Date.UTC(2099, 1, 1))
    expect((await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_buyer" })).plan).toBe("pro")
  })

  it("makes the user Free again when the entitlement is revoked or expires", async () => {
    const t = await makeConvex()
    await t.mutation(api.entitlements.upsert, { ...row, owner: { clerkId: "user_refund" } })
    expect((await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_refund" })).plan).toBe("pro")
    await t.mutation(api.entitlements.upsert, { ...row, status: "revoked" })
    expect((await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_refund" })).plan).toBe("free")
    await t.mutation(api.entitlements.upsert, { ...row, status: "grace", expiresAt: Date.UTC(2099, 0, 1) })
    expect((await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_refund" })).plan).toBe("pro")
    await t.mutation(api.entitlements.upsert, { ...row, status: "active", expiresAt: Date.now() - 1 })
    expect((await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_refund" })).plan).toBe("free")
  })

  it("refuses a notification nobody can be found for", async () => {
    const t = await makeConvex()
    const r = await t.mutation(api.entitlements.upsert, { ...row, owner: { appAccountToken: crypto.randomUUID() } })
    expect(r).toEqual({ ok: false, reason: "unknown_user" })
  })
})

describe("webhooks.claim", () => {
  it("dedupes deliveries per source and event id, and a released claim can be retried", async () => {
    const t = await makeConvex()
    const claim = (source: string, eventId: string) => t.mutation(api.webhooks.claim, { secret: SECRET, source, eventId })
    expect(await claim("stripe", "evt_1")).toEqual({ fresh: true })
    expect(await claim("stripe", "evt_1")).toEqual({ fresh: false })
    expect(await claim("apple", "evt_1")).toEqual({ fresh: true })
    await t.mutation(api.webhooks.release, { secret: SECRET, source: "stripe", eventId: "evt_1" })
    expect(await claim("stripe", "evt_1")).toEqual({ fresh: true })
  })
})

describe("sermons", () => {
  const create = (clerkId: string, durationSec: number) => ({
    secret: SECRET,
    clerkId,
    recordingId: "rec",
    durationSec,
    bytes: durationSec * 32000 + 44,
    nonce: crypto.randomUUID().slice(0, 8)
  })

  it("gives Free one trial sermon of up to an hour, given back if transcription fails", async () => {
    const t = await makeConvex()
    const long = await t.mutation(api.sermons.create, create("user_trial", 3601))
    expect(!long.ok && long.error.type).toBe("plan_required")
    const first = await t.mutation(api.sermons.create, create("user_trial", 1800))
    expect(first.ok && first.trial).toBe(true)
    const second = await t.mutation(api.sermons.create, create("user_trial", 60))
    expect(!second.ok && second.error.type).toBe("plan_required")
    if (!first.ok) throw new Error("denied")
    await t.mutation(api.sermons.fail, { secret: SECRET, sermonId: first.sermonId, error: "boom" })
    const retry = await t.mutation(api.sermons.create, create("user_trial", 60))
    expect(retry.ok).toBe(true)
  })

  it("holds Pro to 10 sermon-hours a month, counting reserved audio, and settles the real duration", async () => {
    const t = await makeConvex()
    await t.mutation(internal.admin.grant, { clerkId: "user_preacher", days: 30 })
    const a = await t.mutation(api.sermons.create, create("user_preacher", 3 * 3600))
    const b = await t.mutation(api.sermons.create, create("user_preacher", 3 * 3600))
    const c = await t.mutation(api.sermons.create, create("user_preacher", 3 * 3600))
    const d = await t.mutation(api.sermons.create, create("user_preacher", 3601))
    expect([a.ok, b.ok, c.ok]).toEqual([true, true, true])
    expect(!d.ok && d.error.type).toBe("allowance_exhausted")
    if (!a.ok) throw new Error("denied")
    // a finishes at 2.5 h of real audio: the half hour reserved beyond it is freed.
    await t.action(api.sermons.complete, { secret: SECRET, sermonId: a.sermonId, transcript: "{}", audioSeconds: 2.5 * 3600 })
    const me = await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_preacher" })
    expect(me.sermons.hoursLeft).toBe(1.5)
    // A second completion (webhook and poll racing) settles nothing more.
    const again = await t.action(api.sermons.complete, { secret: SECRET, sermonId: a.sermonId, transcript: "{}", audioSeconds: 2.5 * 3600 })
    expect(again.done).toBe(false)
    const ledger = await t.run((ctx) => ctx.db.query("ledger").collect())
    expect(ledger.filter((r) => r.kind === "sermon")).toHaveLength(1)
  })
})

describe("account.purge", () => {
  it("deletes every row a user owns and reports what vendors still hold", async () => {
    const t = await makeConvex()
    await t.mutation(api.usage.beginRound, begin("user_gone"))
    const s = await t.mutation(api.sermons.create, {
      secret: SECRET,
      clerkId: "user_gone",
      recordingId: "r",
      durationSec: 60,
      bytes: 60 * 32000 + 44,
      nonce: "abc"
    })
    await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_stays" })
    const left = await t.mutation(api.account.purge, { secret: SECRET, clerkId: "user_gone" })
    expect(left.found).toBe(true)
    expect(left.blobPaths).toEqual([s.ok ? s.blobPath : ""])
    const counts = await t.run(async (ctx) => ({
      users: (await ctx.db.query("users").collect()).map((u) => u.clerkId),
      reservations: (await ctx.db.query("reservations").collect()).length,
      questions: (await ctx.db.query("questions").collect()).length,
      usage: (await ctx.db.query("usage").collect()).length,
      sermons: (await ctx.db.query("sermons").collect()).length
    }))
    expect(counts).toEqual({ users: ["user_stays"], reservations: 0, questions: 0, usage: 0, sermons: 0 })
    expect((await t.mutation(api.account.purge, { secret: SECRET, clerkId: "user_gone" })).found).toBe(false)
  })
})
