import { v } from "convex/values"
import { Effect } from "effect"
import { internal } from "./_generated/api.js"
import { purgeAccount } from "./library/purge.js"
import { userByClerk } from "./lib/accounts.js"
import { effectInternalAction, effectServerMutation } from "./lib/effect.js"
import type { Purge } from "./lib/purge.js"
import { MutationCtx } from "./lib/services.js"
import { Transcriber } from "./lib/transcriber.js"

export interface Purged {
  readonly found: boolean
  readonly stripeCustomerId?: string | undefined
}

/**
 * Starts deleting everything Linger holds about a user (DELETE /v1/account and Clerk's
 * user.deleted webhook): the first step of the purge chain (library/purge.ts) runs here, the
 * rest in scheduled steps, 500 records each. AssemblyAI jobs still open are deleted by
 * `forgetJobs`; the Stripe customer, returned, is the proxy's to delete.
 */
export const purge = effectServerMutation({
  args: { clerkId: v.string() },
  handler: Effect.fn("account.purge")(function*({ clerkId }): Effect.fn.Return<Purged, never, MutationCtx | Purge> {
    const ctx = yield* MutationCtx
    const user = yield* Effect.promise(() => userByClerk(ctx, clerkId))
    if (!user) return { found: false }
    // Read before the first step deletes the sermon rows. A job is deleted once its transcript is
    // kept, so these are the ones still open.
    const sermons = yield* Effect.promise(() => ctx.db.query("sermons").withIndex("by_user", (q) => q.eq("userId", user._id)).collect())
    const assemblyIds = sermons.flatMap((s) => (s.assemblyId && s.status !== "done" ? [s.assemblyId] : []))
    if (assemblyIds.length) yield* Effect.promise(() => ctx.scheduler.runAfter(0, internal.account.forgetJobs, { assemblyIds }))
    yield* purgeAccount(user._id)
    return { found: true, stripeCustomerId: user.stripeCustomerId }
  })
})

/** Deletes a purged account's open AssemblyAI jobs, and with them the provider's copy of the audio. Best effort: the Transcriber logs a failure. */
export const forgetJobs = effectInternalAction({
  args: { assemblyIds: v.array(v.string()) },
  handler: Effect.fn("account.forgetJobs")(function*({ assemblyIds }) {
    const transcriber = yield* Transcriber
    yield* Effect.forEach(assemblyIds, (id) => transcriber.delete(id), { concurrency: 4, discard: true })
  })
})
