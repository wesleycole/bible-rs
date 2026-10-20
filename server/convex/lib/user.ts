/**
 * Functions the apps call with their Clerk token. The identity comes from `ctx.auth`, never from
 * an argument; a mutation creates the `users` row on first sight, a query sees `null` until then.
 */
import { customMutation, customQuery } from "convex-helpers/server/customFunctions"
import { Effect } from "effect"
import type { MutationCtx, QueryCtx } from "../_generated/server.js"
import { mutation, query } from "../_generated/server.js"
import { Unauthenticated } from "../../shared/Errors.js"
import { ensureUser, userByClerk } from "./accounts.js"
import { toConvexError } from "./errors.js"
import { MutationCtx as Mutation } from "./services.js"

const identityOf = async (ctx: QueryCtx) => {
  const identity = await ctx.auth.getUserIdentity()
  if (!identity) throw toConvexError(new Unauthenticated({ message: "Sign in to use your library." }))
  if (!identity.subject.startsWith("user_")) {
    throw toConvexError(new Unauthenticated({ message: "That token isn't a Linger user." }))
  }
  return identity
}

export const userMutation = customMutation(mutation, {
  args: {},
  input: async (ctx: MutationCtx) => {
    const identity = await identityOf(ctx)
    const user = await Effect.runPromise(
      ensureUser(identity.subject, identity.email).pipe(Effect.provideService(Mutation, ctx))
    )
    return { ctx: { user }, args: {} }
  }
})

export const userQuery = customQuery(query, {
  args: {},
  input: async (ctx: QueryCtx) => {
    const identity = await identityOf(ctx)
    const user = await userByClerk(ctx, identity.subject)
    return { ctx: { user }, args: {} }
  }
})
