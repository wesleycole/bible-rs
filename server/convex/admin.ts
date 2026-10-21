/**
 * Operator functions, run from the dashboard or `npx convex run`. Nothing here is callable by the
 * proxy or the app.
 */
import { v } from "convex/values"
import { Clock, Effect } from "effect"
import { ensureUser } from "./lib/accounts.js"
import { effectInternalMutation, effectInternalQuery } from "./lib/effect.js"
import { MutationCtx, QueryCtx } from "./lib/services.js"
import { DEFAULT_PRICES } from "./lib/pricing.js"
import { DAY } from "./lib/time.js"

const PLANS = [
  {
    key: "free",
    questionsPerMonth: 30,
    questionsPerDay: 10,
    deepCost: 2,
    maxRounds: 6,
    costCeilingMicros: 250_000,
    sermonSecondsPerMonth: 0,
    modelKey: "free",
    storageBytes: 1_000_000_000,
    keepAudio: false
  },
  {
    key: "pro",
    questionsPerMonth: 150,
    questionsPerDay: 150,
    deepCost: 2,
    maxRounds: 8,
    costCeilingMicros: 5_000_000,
    sermonSecondsPerMonth: 10 * 3600,
    modelKey: "standard",
    deepModelKey: "deep",
    ceilingModelKey: "free",
    storageBytes: 25_000_000_000,
    keepAudio: true
  }
] as const

const MODELS = [
  { key: "free", gatewayModel: "anthropic/claude-haiku-5.5", thinking: "adaptive", effort: "low", maxTokens: 4000 },
  { key: "standard", gatewayModel: "anthropic/claude-sonnet-5.5", thinking: "adaptive", effort: "medium", maxTokens: 8000 },
  { key: "deep", gatewayModel: "anthropic/claude-opus-5.5", thinking: "adaptive", effort: "high", maxTokens: 12000 },
  /** The sermon digest (library/digest.ts): references, an outline and a summary as JSON, no thinking. */
  { key: "digest", gatewayModel: "anthropic/claude-haiku-5.5", thinking: "disabled", effort: "low", maxTokens: 4000 }
] as const

/**
 * Seeds the plans, models and routing rows. Existing rows are left alone unless `overwrite`, so
 * values retuned in the dashboard survive a re-seed; a field a plan row lacks (one added since it
 * was seeded) is filled in either way.
 */
export const seed = effectInternalMutation({
  args: { overwrite: v.optional(v.boolean()) },
  handler: Effect.fn("admin.seed")(function*({ overwrite }) {
    const ctx = yield* MutationCtx
    const changed: string[] = []
    for (const plan of PLANS) {
      const row = yield* Effect.promise(() => ctx.db.query("plans").withIndex("by_key", (q) => q.eq("key", plan.key)).unique())
      if (!row) {
        yield* Effect.promise(() => ctx.db.insert("plans", { ...plan }))
        changed.push(`plans.${plan.key}`)
      } else if (overwrite) {
        yield* Effect.promise(() => ctx.db.replace(row._id, { ...plan }))
        changed.push(`plans.${plan.key}`)
      } else {
        const missing = Object.fromEntries(Object.entries(plan).filter(([k]) => !(k in row)))
        if (Object.keys(missing).length > 0) {
          yield* Effect.promise(() => ctx.db.patch(row._id, missing))
          changed.push(...Object.keys(missing).map((k) => `plans.${plan.key}.${k}`))
        }
      }
    }
    for (const m of MODELS) {
      const model = { ...m, enabled: true, prices: DEFAULT_PRICES[m.gatewayModel] }
      const row = yield* Effect.promise(() => ctx.db.query("models").withIndex("by_key", (q) => q.eq("key", m.key)).unique())
      if (!row) {
        yield* Effect.promise(() => ctx.db.insert("models", model))
        changed.push(`models.${m.key}`)
      } else if (overwrite) {
        yield* Effect.promise(() => ctx.db.replace(row._id, model))
        changed.push(`models.${m.key}`)
      }
    }
    const routing = yield* Effect.promise(() => ctx.db.query("routing").first())
    const defaults = { providers: ["anthropic"], note: "default" }
    if (!routing) {
      yield* Effect.promise(() => ctx.db.insert("routing", defaults))
      changed.push("routing")
    } else if (overwrite) {
      yield* Effect.promise(() => ctx.db.replace(routing._id, defaults))
      changed.push("routing")
    }
    return { changed }
  })
})

/** Points new questions at other providers (an outage drill or a real outage). */
export const setRouting = effectInternalMutation({
  args: { providers: v.array(v.string()), note: v.string() },
  handler: Effect.fn("admin.setRouting")(function*(args) {
    const ctx = yield* MutationCtx
    const routing = yield* Effect.promise(() => ctx.db.query("routing").first())
    if (routing) yield* Effect.promise(() => ctx.db.replace(routing._id, args))
    else yield* Effect.promise(() => ctx.db.insert("routing", args))
  })
})

/** Comps, press and family: Pro for `days` days. */
export const grant = effectInternalMutation({
  args: { clerkId: v.string(), days: v.number(), note: v.optional(v.string()) },
  handler: Effect.fn("admin.grant")(function*({ clerkId, days, note }) {
    const ctx = yield* MutationCtx
    const user = yield* ensureUser(clerkId)
    const now = yield* Clock.currentTimeMillis
    return yield* Effect.promise(() => ctx.db.insert("entitlements", {
      userId: user._id,
      source: "grant",
      productId: note ?? "grant",
      status: "active",
      periodStart: now,
      expiresAt: now + days * DAY,
      originalId: `grant:${user._id}:${now}`,
      environment: "production",
      updatedAt: now
    }))
  })
})

/** Open reservations and what the ledger recorded since `sinceMs`, by status: a settlement check. */
export const stats = effectInternalQuery({
  args: { sinceMs: v.number() },
  handler: Effect.fn("admin.stats")(function*({ sinceMs }) {
    const ctx = yield* QueryCtx
    const open = yield* Effect.promise(() => ctx.db.query("reservations").collect())
    const ledger = yield* Effect.promise(() => ctx.db.query("ledger").withIndex("by_at", (q) => q.gte("at", sinceMs)).collect())
    const byStatus: Record<string, number> = {}
    for (const row of ledger) byStatus[row.status] = (byStatus[row.status] ?? 0) + 1
    return {
      openReservations: open.length,
      ledgerRows: ledger.length,
      byStatus,
      costMicros: ledger.reduce((sum, row) => sum + row.costMicros, 0)
    }
  })
})
