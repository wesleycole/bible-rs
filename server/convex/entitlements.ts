import { v } from "convex/values"
import { Clock, Effect } from "effect"
import type { Doc, Id } from "./_generated/dataModel.js"
import type { MutationCtx as RawMutationCtx } from "./_generated/server.js"
import { ensureUser } from "./lib/accounts.js"
import { effectServerMutation } from "./lib/effect.js"
import { MutationCtx } from "./lib/services.js"

const source = v.union(v.literal("app_store"), v.literal("stripe"), v.literal("grant"))
const status = v.union(
  v.literal("active"),
  v.literal("grace"),
  v.literal("billing_retry"),
  v.literal("expired"),
  v.literal("revoked"),
  v.literal("canceled")
)

/** Who an entitlement belongs to, as each source knows them. */
const owner = v.union(
  v.object({ clerkId: v.string() }),
  v.object({ appAccountToken: v.string() }),
  v.object({ stripeCustomerId: v.string() })
)

const findOwner = async (
  ctx: RawMutationCtx,
  who: { clerkId: string } | { appAccountToken: string } | { stripeCustomerId: string }
): Promise<Id<"users"> | null> => {
  if ("clerkId" in who) return (await ensureUser(ctx, who.clerkId))._id
  if ("appAccountToken" in who) {
    const u = await ctx.db
      .query("users")
      .withIndex("by_app_account_token", (q) => q.eq("appAccountToken", who.appAccountToken))
      .unique()
    return u?._id ?? null
  }
  const u = await ctx.db
    .query("users")
    .withIndex("by_stripe_customer", (q) => q.eq("stripeCustomerId", who.stripeCustomerId))
    .unique()
  return u?._id ?? null
}

/**
 * Records the current state of one subscription, keyed by its source and original id, so a
 * notification delivered twice (or out of order with the client's own post) lands once. A later
 * notification for the same subscription finds its owner through the existing row.
 */
export const upsert = effectServerMutation({
  args: {
    owner: v.optional(owner),
    source,
    productId: v.string(),
    status,
    periodStart: v.number(),
    expiresAt: v.number(),
    originalId: v.string(),
    environment: v.string()
  },
  handler: Effect.fn("entitlements.upsert")(function*(args) {
    const ctx = yield* MutationCtx
    const existing = yield* Effect.promise(() =>
      ctx.db.query("entitlements").withIndex("by_original", (q) => q.eq("source", args.source).eq("originalId", args.originalId)).unique()
    )
    const fromOwner = args.owner ? yield* Effect.promise(() => findOwner(ctx, args.owner!)) : null
    const userId = fromOwner ?? existing?.userId ?? null
    if (!userId) return { ok: false as const, reason: "unknown_user" as const }
    const row: Omit<Doc<"entitlements">, "_id" | "_creationTime"> = {
      userId,
      source: args.source,
      productId: args.productId,
      status: args.status,
      periodStart: args.periodStart,
      expiresAt: args.expiresAt,
      originalId: args.originalId,
      environment: args.environment,
      updatedAt: yield* Clock.currentTimeMillis
    }
    if (existing) {
      yield* Effect.promise(() => ctx.db.patch(existing._id, row))
      return { ok: true as const, entitlementId: existing._id, created: false }
    }
    return { ok: true as const, entitlementId: yield* Effect.promise(() => ctx.db.insert("entitlements", row)), created: true }
  })
})
