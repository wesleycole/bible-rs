/**
 * The services a Convex handler runs over. Handlers ask for these; nothing reaches `ctx` another
 * way, so a handler's dependencies are in its type and a test can provide fakes.
 */
import { Context } from "effect"
import type { Id } from "../_generated/dataModel.js"
import type { MutationCtx as RawMutationCtx, QueryCtx as RawQueryCtx } from "../_generated/server.js"

/** The handler's `ctx` in a query, or the reading half of a mutation's. */
export class QueryCtx extends Context.Service<QueryCtx, RawQueryCtx>()("linger/convex/QueryCtx") {}

/** The handler's `ctx` in a mutation. */
export class MutationCtx extends Context.Service<MutationCtx, RawMutationCtx>()("linger/convex/MutationCtx") {}

/** The signed-in user, from the Clerk identity Convex verified. A user id never comes from an argument. */
export class CurrentUser extends Context.Service<CurrentUser, {
  readonly clerkId: string
  readonly userId: Id<"users">
}>()("linger/convex/CurrentUser") {}
