import { v } from "convex/values"
import { Effect } from "effect"
import { purgeSome } from "./library/purge.js"
import { userByClerk } from "./lib/accounts.js"
import { effectServerMutation } from "./lib/effect.js"
import { MutationCtx } from "./lib/services.js"

/**
 * Starts deleting everything Linger holds about a user (DELETE /v1/account and Clerk's
 * user.deleted webhook): the first step of the purge chain (library/purge.ts) runs here, the
 * rest in scheduled steps, 500 records each.
 * Returns what vendors still hold, which the proxy deletes: blobs, AssemblyAI jobs and the Stripe
 * customer.
 */
export const purge = effectServerMutation({
  args: { clerkId: v.string() },
  handler: Effect.fn("account.purge")(function*({ clerkId }) {
    const ctx = yield* MutationCtx
    return yield* Effect.promise(async () => {
      const user = await userByClerk(ctx, clerkId)
      if (!user) return { found: false, blobPaths: [] as string[], assemblyIds: [] as string[] }
      const sermons = await ctx.db.query("sermons").withIndex("by_user", (q) => q.eq("userId", user._id)).collect()
      await purgeSome(ctx, user._id)
      return {
        found: true,
        stripeCustomerId: user.stripeCustomerId,
        blobPaths: sermons.filter((s) => s.status !== "done").map((s) => s.blobPath),
        assemblyIds: sermons.flatMap((s) => (s.assemblyId ? [s.assemblyId] : []))
      }
    })
  })
})
