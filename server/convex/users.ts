import { v } from "convex/values"
import { effectivePlan, ensureUser, findUsage, loadPlan, outstanding, userByClerk } from "./lib/accounts.js"
import { serverMutation, serverQuery } from "./lib/server.js"
import { nextPeriodStart } from "./lib/time.js"

/** Everything GET /v1/me shows: the plan, what is left of it, and the StoreKit account token. */
export const me = serverMutation({
  args: { clerkId: v.string(), email: v.optional(v.string()) },
  handler: async (ctx, { clerkId, email }) => {
    const now = Date.now()
    const user = await ensureUser(ctx, clerkId, email)
    const { plan: planKey, entitlement } = await effectivePlan(ctx, user._id, now)
    const plan = await loadPlan(ctx, planKey)
    const usage = await findUsage(ctx, user._id, now)
    const held = await outstanding(ctx, user._id)
    const sermonSecondsLeft = Math.max(0, plan.sermonSecondsPerMonth - (usage?.sermonSeconds ?? 0) - held.seconds)
    return {
      user: { id: clerkId, email: user.email, appAccountToken: user.appAccountToken },
      plan: planKey,
      renewsAt: entitlement ? new Date(entitlement.expiresAt).toISOString() : undefined,
      source: entitlement && entitlement.source !== "grant" ? entitlement.source : undefined,
      ask: {
        questionsLeft: Math.max(0, plan.questionsPerMonth - (usage?.questions ?? 0)),
        resetsAt: new Date(nextPeriodStart(now)).toISOString(),
        deepAvailable: planKey === "pro" && plan.deepModelKey !== undefined
      },
      sermons: {
        hoursLeft: Math.round((sermonSecondsLeft / 3600) * 100) / 100,
        trialAvailable: planKey === "free" && !user.trialHourUsed
      }
    }
  }
})

export const setStripeCustomer = serverMutation({
  args: { clerkId: v.string(), stripeCustomerId: v.string() },
  handler: async (ctx, { clerkId, stripeCustomerId }) => {
    const user = await ensureUser(ctx, clerkId)
    await ctx.db.patch(user._id, { stripeCustomerId })
    return { userId: user._id }
  }
})

/** The user behind a Clerk id, for billing: their Convex id and Stripe customer, if any. */
export const billingProfile = serverQuery({
  args: { clerkId: v.string() },
  handler: async (ctx, { clerkId }) => {
    const user = await userByClerk(ctx, clerkId)
    return user
      ? { userId: user._id, email: user.email, stripeCustomerId: user.stripeCustomerId, appAccountToken: user.appAccountToken }
      : null
  }
})
