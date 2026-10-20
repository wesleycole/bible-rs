import { Clock, Effect } from "effect"
import type { Doc, Id } from "../_generated/dataModel.js"
import type { MutationCtx, QueryCtx } from "../_generated/server.js"
import { MutationCtx as Mutation } from "./services.js"
import { dayOf, periodOf } from "./time.js"

export type PlanKey = "free" | "pro"

export const userByClerk = (ctx: QueryCtx, clerkId: string) =>
  ctx.db.query("users").withIndex("by_clerk", (q) => q.eq("clerkId", clerkId)).unique()

/** The user for a verified Clerk id, created on first sight with its StoreKit account token. */
export const ensureUser = Effect.fn("ensureUser")(function*(
  clerkId: string,
  email?: string
): Effect.fn.Return<Doc<"users">, never, Mutation> {
  const ctx = yield* Mutation
  const existing = yield* Effect.promise(() => userByClerk(ctx, clerkId))
  if (existing) {
    if (email && existing.email !== email) {
      yield* Effect.promise(() => ctx.db.patch(existing._id, { email }))
      return { ...existing, email }
    }
    return existing
  }
  const createdAt = yield* Clock.currentTimeMillis
  const id = yield* Effect.promise(() =>
    ctx.db.insert("users", { clerkId, email, createdAt, appAccountToken: crypto.randomUUID(), trialHourUsed: false })
  )
  return (yield* Effect.promise(() => ctx.db.get(id)))!
})

/** An entitlement counts while it is active or in its billing grace period, and unexpired. */
export const isEntitled = (e: Doc<"entitlements">, now: number) =>
  (e.status === "active" || e.status === "grace") && e.expiresAt > now

/** Pro when any entitlement is current, from any source; otherwise Free. */
export const effectivePlan = async (
  ctx: QueryCtx,
  userId: Id<"users">,
  now: number
): Promise<{ plan: PlanKey; entitlement: Doc<"entitlements"> | null }> => {
  const rows = await ctx.db.query("entitlements").withIndex("by_user", (q) => q.eq("userId", userId)).collect()
  const current = rows.filter((e) => isEntitled(e, now)).sort((a, b) => b.expiresAt - a.expiresAt)
  return current.length > 0 ? { plan: "pro", entitlement: current[0] } : { plan: "free", entitlement: null }
}

export const loadPlan = async (ctx: QueryCtx, key: PlanKey): Promise<Doc<"plans">> => {
  const plan = await ctx.db.query("plans").withIndex("by_key", (q) => q.eq("key", key)).unique()
  if (!plan) throw new Error(`plan ${key} is not seeded; run admin:seed`)
  return plan
}

export const loadModel = (ctx: QueryCtx, key: string) =>
  ctx.db.query("models").withIndex("by_key", (q) => q.eq("key", key)).unique()

/** This month's usage row, as stored (null before the first use this month). */
export const findUsage = (ctx: QueryCtx, userId: Id<"users">, now: number) =>
  ctx.db
    .query("usage")
    .withIndex("by_user_period", (q) => q.eq("userId", userId).eq("period", periodOf(now)))
    .unique()

/** This month's usage row, created when missing, with the daily count rolled over at UTC midnight. */
export const ensureUsage = async (ctx: MutationCtx, userId: Id<"users">, now: number): Promise<Doc<"usage">> => {
  const row = await findUsage(ctx, userId, now)
  const today = dayOf(now)
  if (!row) {
    const id = await ctx.db.insert("usage", {
      userId,
      period: periodOf(now),
      questions: 0,
      deepQuestions: 0,
      costMicros: 0,
      sermonSeconds: 0,
      day: today,
      questionsToday: 0
    })
    return (await ctx.db.get(id))!
  }
  if (row.day !== today) {
    await ctx.db.patch(row._id, { day: today, questionsToday: 0 })
    return { ...row, day: today, questionsToday: 0 }
  }
  return row
}

/** Money and audio reserved but not yet settled. */
export const outstanding = async (ctx: QueryCtx, userId: Id<"users">) => {
  const rows = await ctx.db.query("reservations").withIndex("by_user", (q) => q.eq("userId", userId)).collect()
  return {
    micros: rows.reduce((sum, r) => sum + (r.amountMicros ?? 0), 0),
    seconds: rows.reduce((sum, r) => sum + (r.seconds ?? 0), 0)
  }
}
