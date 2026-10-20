import { v } from "convex/values"
import { Effect } from "effect"
import { purgeAccount } from "./library/purge.js"
import { userByClerk } from "./lib/accounts.js"
import { effectServerMutation } from "./lib/effect.js"
import type { Purge } from "./lib/purge.js"
import { MutationCtx } from "./lib/services.js"

export interface Purged {
  readonly found: boolean
  readonly blobPaths: Array<string>
  readonly assemblyIds: Array<string>
  readonly stripeCustomerId?: string | undefined
}

/**
 * Starts deleting everything Linger holds about a user (DELETE /v1/account and Clerk's
 * user.deleted webhook): the first step of the purge chain (library/purge.ts) runs here, the
 * rest in scheduled steps, 500 records each.
 * Returns what vendors still hold, which the proxy deletes: blobs, AssemblyAI jobs and the Stripe
 * customer.
 */
export const purge = effectServerMutation({
  args: { clerkId: v.string() },
  handler: Effect.fn("account.purge")(function*({ clerkId }): Effect.fn.Return<Purged, never, MutationCtx | Purge> {
    const ctx = yield* MutationCtx
    const user = yield* Effect.promise(() => userByClerk(ctx, clerkId))
    if (!user) return { found: false, blobPaths: [] as string[], assemblyIds: [] as string[] }
    const sermons = yield* Effect.promise(() => ctx.db.query("sermons").withIndex("by_user", (q) => q.eq("userId", user._id)).collect())
    // What vendors hold is read before the first step deletes the sermon rows.
    const left = {
      found: true,
      stripeCustomerId: user.stripeCustomerId,
      blobPaths: sermons.flatMap((s) => (s.status !== "done" && s.blobPath ? [s.blobPath] : [])),
      assemblyIds: sermons.flatMap((s) => (s.assemblyId ? [s.assemblyId] : []))
    }
    yield* purgeAccount(user._id)
    return left
  })
})
