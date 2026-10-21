/**
 * Functions the apps call with their Clerk token. The identity comes from `ctx.auth`, never from
 * an argument; a mutation creates the `users` row on first sight, a query sees `null` until then.
 */
import { customAction, customMutation, customQuery } from "convex-helpers/server/customFunctions"
import { makeFunctionReference } from "convex/server"
import { Effect } from "effect"
import type { Doc } from "../_generated/dataModel.js"
import type { ActionCtx, MutationCtx, QueryCtx } from "../_generated/server.js"
import { action, mutation, query } from "../_generated/server.js"
import { Unauthenticated } from "../../shared/Errors.js"
import { ensureUser, userByClerk } from "./accounts.js"
import { toConvexError } from "./errors.js"
import { MutationCtx as Mutation } from "./services.js"

const identityOf = async (ctx: Pick<QueryCtx, "auth">) => {
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

/** `whoami.user` by name: naming it through `internal` would make this module's type depend on every function's, its own included. */
const whoamiUser = makeFunctionReference<"query", { clerkId: string }, Doc<"users"> | null>("library/whoami:user")

/** An action the apps call (search, export): the `users` row comes through `whoami.user`, and is `null` until the first write, as for a query. */
export const userAction = customAction(action, {
  args: {},
  input: async (ctx: ActionCtx): Promise<{ ctx: { user: Doc<"users"> | null }; args: {} }> => {
    const identity = await identityOf(ctx)
    const user = await ctx.runQuery(whoamiUser, { clerkId: identity.subject })
    return { ctx: { user }, args: {} }
  }
})
