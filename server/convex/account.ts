import { v } from "convex/values"
import type { Id } from "./_generated/dataModel.js"
import type { MutationCtx } from "./_generated/server.js"
import { userByClerk } from "./lib/accounts.js"
import { serverMutation } from "./lib/server.js"

/**
 * Deletes everything Linger holds about a user (DELETE /v1/account and Clerk's user.deleted
 * webhook). Returns the blobs and AssemblyAI jobs the proxy must delete, and the Stripe customer.
 */
export const purge = serverMutation({
  args: { clerkId: v.string() },
  handler: async (ctx, { clerkId }) => {
    const user = await userByClerk(ctx, clerkId)
    if (!user) return { found: false, blobPaths: [] as string[], assemblyIds: [] as string[] }
    const sermons = await ctx.db.query("sermons").withIndex("by_user", (q) => q.eq("userId", user._id)).collect()
    for (const s of sermons) {
      if (s.transcript) await ctx.storage.delete(s.transcript)
      await ctx.db.delete(s._id)
    }
    await deleteAll(ctx, "entitlements", user._id)
    await deleteAll(ctx, "usage", user._id)
    await deleteAll(ctx, "reservations", user._id)
    await deleteAll(ctx, "ledger", user._id)
    await deleteAll(ctx, "questions", user._id)
    await ctx.db.delete(user._id)
    return {
      found: true,
      stripeCustomerId: user.stripeCustomerId,
      blobPaths: sermons.filter((s) => s.status !== "done").map((s) => s.blobPath),
      assemblyIds: sermons.flatMap((s) => (s.assemblyId ? [s.assemblyId] : []))
    }
  }
})

type Owned = "entitlements" | "usage" | "reservations" | "ledger" | "questions"

const deleteAll = async (ctx: MutationCtx, table: Owned, userId: Id<"users">) => {
  // Small per-user tables, except the ledger and questions, which have no user-only index and are
  // read through a filter; a user's rows there number in the hundreds at most.
  const rows = await ctx.db
    .query(table)
    .filter((q) => q.eq(q.field("userId"), userId))
    .collect()
  for (const r of rows) await ctx.db.delete(r._id)
}
