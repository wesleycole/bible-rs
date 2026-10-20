import { v } from "convex/values"
import { Effect } from "effect"
import { ensureUser, userByClerk } from "./lib/accounts.js"
import { Allowance } from "./lib/allowance.js"
import { effectServerMutation, effectServerQuery } from "./lib/effect.js"
import { MutationCtx, QueryCtx } from "./lib/services.js"

/** Everything GET /v1/me shows: the plan, what is left of it, library storage, and the StoreKit account token. */
export const me = effectServerMutation({
  args: { clerkId: v.string(), email: v.optional(v.string()) },
  handler: Effect.fn("users.me")(function*({ clerkId, email }) {
    const ctx = yield* MutationCtx
    const user = yield* Effect.promise(() => ensureUser(ctx, clerkId, email))
    const summary = yield* (yield* Allowance).summary(user)
    return { user: { id: clerkId, email: user.email, appAccountToken: user.appAccountToken }, ...summary }
  })
})

export const setStripeCustomer = effectServerMutation({
  args: { clerkId: v.string(), stripeCustomerId: v.string() },
  handler: Effect.fn("users.setStripeCustomer")(function*({ clerkId, stripeCustomerId }) {
    const ctx = yield* MutationCtx
    const user = yield* Effect.promise(() => ensureUser(ctx, clerkId))
    yield* Effect.promise(() => ctx.db.patch(user._id, { stripeCustomerId }))
    return { userId: user._id }
  })
})

/** The user behind a Clerk id, for billing: their Convex id and Stripe customer, if any. */
export const billingProfile = effectServerQuery({
  args: { clerkId: v.string() },
  handler: Effect.fn("users.billingProfile")(function*({ clerkId }) {
    const ctx = yield* QueryCtx
    const user = yield* Effect.promise(() => userByClerk(ctx, clerkId))
    return user
      ? { userId: user._id, email: user.email, stripeCustomerId: user.stripeCustomerId, appAccountToken: user.appAccountToken }
      : null
  })
})
