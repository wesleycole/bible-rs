/**
 * Effect inside Convex. A handler is an Effect program over the services in services.ts: its
 * arguments are decoded with a Schema at the edge, its dependencies are in its type, and a typed
 * failure from shared/Errors.ts becomes the `ConvexError` the apps already understand. Anything
 * else is a defect and is rethrown, so Convex logs it as the bug it is.
 */
import { Cause, Effect, Exit, Layer, Option, Schema } from "effect"
import type { Doc } from "../_generated/dataModel.js"
import type { MutationCtx as RawMutationCtx, QueryCtx as RawQueryCtx } from "../_generated/server.js"
import { type ApiError, InvalidRequest } from "../../shared/Errors.js"
import { toConvexError } from "./errors.js"
import { Anchors } from "./anchors.js"
import { Files } from "./files.js"
import { Sequence } from "./sequence.js"
import { CurrentUser, MutationCtx, QueryCtx } from "./services.js"
import { userMutation, userQuery } from "./user.js"

/** Runs a handler's effect: a typed failure becomes a ConvexError, a defect stays thrown. */
export const run = async <A>(effect: Effect.Effect<A, ApiError>): Promise<A> => {
  const exit = await Effect.runPromiseExit(effect)
  if (Exit.isSuccess(exit)) return exit.value
  const failure = Cause.findErrorOption(exit.cause)
  if (Option.isSome(failure)) throw toConvexError(failure.value)
  throw Cause.squash(exit.cause)
}

/** An argument schema: any codec that decodes without services. */
export type Args = Schema.Codec<any, any, never, never>

/** Decodes a function's arguments; a bad shape is the client's bug, an `invalid_request`. */
const decoder = <S extends Args>(schema: S) => {
  const decode = Schema.decodeUnknownEffect(schema)
  return (raw: unknown) =>
    decode(raw).pipe(Effect.mapError((e) => new InvalidRequest({ message: `The arguments are invalid: ${e.message}` })))
}

type WithUser<Ctx> = Ctx & { readonly user: Doc<"users"> }
type MaybeUser<Ctx> = Ctx & { readonly user: Doc<"users"> | null }

const currentUser = (user: Doc<"users">) => CurrentUser.of({ clerkId: user.clerkId, userId: user._id })

const queryServices = (ctx: WithUser<RawQueryCtx>) =>
  Layer.mergeAll(Layer.succeed(QueryCtx, QueryCtx.of(ctx)), Layer.succeed(CurrentUser, currentUser(ctx.user)))

const mutationServices = (ctx: WithUser<RawMutationCtx>) =>
  Layer.mergeAll(Files.layer, Anchors.layer).pipe(
    Layer.provideMerge(Sequence.layer),
    Layer.provideMerge(
      Layer.mergeAll(
        Layer.succeed(MutationCtx, MutationCtx.of(ctx)),
        Layer.succeed(QueryCtx, QueryCtx.of(ctx)),
        Layer.succeed(CurrentUser, currentUser(ctx.user))
      )
    )
  )

/**
 * A query the apps call with their Clerk token, written as an Effect. A signed-in user whose
 * `users` row doesn't exist yet (nothing has been written) gets `empty(args)`, so a device can
 * start from the change feed without a mutation first.
 */
export const effectQuery = <S extends Args, A>(spec: {
  readonly args: S
  readonly empty: (args: S["Type"]) => Effect.Effect<A>
  readonly handler: (args: S["Type"]) => Effect.Effect<A, ApiError, QueryCtx | CurrentUser>
}) => {
  const decode = decoder(spec.args)
  return userQuery({
    handler: (ctx: MaybeUser<RawQueryCtx>, raw: Record<string, unknown>) =>
      run(
        decode(raw).pipe(
          Effect.flatMap((args) => ctx.user === null ? spec.empty(args) : spec.handler(args).pipe(Effect.provide(queryServices({ ...ctx, user: ctx.user }))))
        )
      )
  })
}

/** What a library mutation's handler can ask for. */
export type MutationServices = MutationCtx | QueryCtx | CurrentUser | Sequence | Files | Anchors

/** A mutation the apps call with their Clerk token, written as an Effect. */
export const effectMutation = <S extends Args, A>(spec: {
  readonly args: S
  readonly handler: (args: S["Type"]) => Effect.Effect<A, ApiError, MutationServices>
}) => {
  const decode = decoder(spec.args)
  return userMutation({
    handler: (ctx, raw: Record<string, unknown>) =>
      run(decode(raw).pipe(Effect.flatMap(spec.handler), Effect.provide(mutationServices(ctx))))
  })
}
