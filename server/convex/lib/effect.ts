/**
 * Effect inside Convex. A handler is an Effect program over the services in services.ts: its
 * arguments are decoded with a Schema at the edge, its dependencies are in its type, and a typed
 * failure from shared/Errors.ts becomes the `ConvexError` the apps already understand. Anything
 * else is a defect and is rethrown, so Convex logs it as the bug it is.
 */
import type { ObjectType, PropertyValidators } from "convex/values"
import { Cause, Effect, Exit, Layer, Option, Schema } from "effect"
import type { Doc } from "../_generated/dataModel.js"
import {
  internalAction,
  internalMutation,
  internalQuery,
  type ActionCtx as RawActionCtx,
  type MutationCtx as RawMutationCtx,
  type QueryCtx as RawQueryCtx
} from "../_generated/server.js"
import { type ApiError, InvalidRequest } from "../../shared/Errors.js"
import { Allowance } from "./allowance.js"
import { Purge } from "./purge.js"
import { toConvexError } from "./errors.js"
import { Anchors } from "./anchors.js"
import { Digest } from "./digest.js"
import { Files } from "./files.js"
import { Sequence } from "./sequence.js"
import { serverAction, serverMutation, serverQuery } from "./server.js"
import { ActionCtx, CurrentUser, MutationCtx, QueryCtx, ServerCaller } from "./services.js"
import { Transcriber } from "./transcriber.js"
import { userAction, userMutation, userQuery } from "./user.js"

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

const mutationCtx = (ctx: RawMutationCtx) =>
  Layer.mergeAll(Layer.succeed(MutationCtx, MutationCtx.of(ctx)), Layer.succeed(QueryCtx, QueryCtx.of(ctx)))

/**
 * The library services acting for `user`, over a mutation's `MutationCtx` and `Allowance`: what a
 * user's own mutation gets, and what an internal mutation (a workflow step, a cron) provides
 * itself when it writes to a user's library.
 */
export const userServices = (user: Doc<"users">) =>
  Layer.mergeAll(Files.layer, Anchors.layer).pipe(
    Layer.provideMerge(Sequence.layer),
    Layer.provideMerge(Layer.succeed(CurrentUser, currentUser(user)))
  )

const mutationServices = (ctx: WithUser<RawMutationCtx>) =>
  userServices(ctx.user).pipe(Layer.provideMerge(Allowance.layer), Layer.provideMerge(mutationCtx(ctx)))

const proxy = Layer.succeed(ServerCaller, ServerCaller.of({ proxy: true }))

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
export type MutationServices = MutationCtx | QueryCtx | CurrentUser | Sequence | Files | Anchors | Allowance

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

/**
 * An action the apps call with their Clerk token (search, export), written as an Effect. As for a
 * query, a signed-in user with no `users` row yet gets `empty(args)`.
 */
export const effectAction = <S extends Args, A>(spec: {
  readonly args: S
  readonly empty: (args: S["Type"]) => Effect.Effect<A>
  readonly handler: (args: S["Type"]) => Effect.Effect<A, ApiError, ActionCtx | CurrentUser>
}) => {
  const decode = decoder(spec.args)
  return userAction({
    handler: (ctx: MaybeUser<RawActionCtx>, raw: Record<string, unknown>) =>
      run(
        decode(raw).pipe(
          Effect.flatMap((args) =>
            ctx.user === null
              ? spec.empty(args)
              : spec.handler(args).pipe(Effect.provide(Layer.mergeAll(Layer.succeed(ActionCtx, ActionCtx.of(ctx)), Layer.succeed(CurrentUser, currentUser(ctx.user)))))
          )
        )
      )
  })
}

/** What a proxy mutation's handler can ask for: it acts for the user the proxy names. */
export type ServerMutationServices = MutationCtx | QueryCtx | ServerCaller | Allowance | Purge

/**
 * A mutation only the proxy calls (it passes the server secret), written as an Effect. Its
 * arguments keep their Convex validators: they are the proxy's typed contract (`Store`).
 */
export const effectServerMutation = <V extends PropertyValidators, A>(spec: {
  readonly args: V
  readonly handler: (args: ObjectType<V>) => Effect.Effect<A, ApiError, ServerMutationServices>
}) =>
  serverMutation({
    args: spec.args,
    handler: (ctx: RawMutationCtx, args: ObjectType<V>): Promise<A> =>
      run(spec.handler(args).pipe(Effect.provide(Layer.mergeAll(Allowance.layer, Purge.layer).pipe(Layer.provideMerge(Layer.mergeAll(mutationCtx(ctx), proxy))))))
  })

/** A query only the proxy calls, written as an Effect. */
export const effectServerQuery = <V extends PropertyValidators, A>(spec: {
  readonly args: V
  readonly handler: (args: ObjectType<V>) => Effect.Effect<A, ApiError, QueryCtx | ServerCaller>
}) =>
  serverQuery({
    args: spec.args,
    handler: (ctx: RawQueryCtx, args: ObjectType<V>): Promise<A> =>
      run(spec.handler(args).pipe(Effect.provide(Layer.mergeAll(Layer.succeed(QueryCtx, QueryCtx.of(ctx)), proxy))))
  })

/** An action only the proxy calls, written as an Effect. */
export const effectServerAction = <V extends PropertyValidators, A>(spec: {
  readonly args: V
  readonly handler: (args: ObjectType<V>) => Effect.Effect<A, ApiError, ActionCtx | ServerCaller>
}) =>
  serverAction({
    args: spec.args,
    handler: (ctx: RawActionCtx, args: ObjectType<V>): Promise<A> =>
      run(spec.handler(args).pipe(Effect.provide(Layer.mergeAll(Layer.succeed(ActionCtx, ActionCtx.of(ctx)), proxy))))
  })

/** What an internal mutation (a cron, a scheduled step) can ask for. */
export type InternalMutationServices = MutationCtx | QueryCtx | Allowance | Purge

/** A mutation nothing outside the deployment can call: crons and scheduled steps. */
export const effectInternalMutation = <V extends PropertyValidators, A>(spec: {
  readonly args: V
  readonly handler: (args: ObjectType<V>) => Effect.Effect<A, ApiError, InternalMutationServices>
}) =>
  internalMutation({
    args: spec.args,
    handler: (ctx: RawMutationCtx, args: ObjectType<V>): Promise<A> =>
      run(spec.handler(args).pipe(Effect.provide(Layer.mergeAll(Allowance.layer, Purge.layer).pipe(Layer.provideMerge(mutationCtx(ctx))))))
  })

/** A query nothing outside the deployment can call: the operator's reports. */
export const effectInternalQuery = <V extends PropertyValidators, A>(spec: {
  readonly args: V
  readonly handler: (args: ObjectType<V>) => Effect.Effect<A, ApiError, QueryCtx>
}) =>
  internalQuery({
    args: spec.args,
    handler: (ctx: RawQueryCtx, args: ObjectType<V>): Promise<A> =>
      run(spec.handler(args).pipe(Effect.provide(Layer.succeed(QueryCtx, QueryCtx.of(ctx)))))
  })

/** An action nothing outside the deployment can call: crons and workflow steps that reach a vendor. */
export const effectInternalAction = <V extends PropertyValidators, A>(spec: {
  readonly args: V
  readonly handler: (args: ObjectType<V>) => Effect.Effect<A, ApiError, ActionCtx | Transcriber | Digest>
}) =>
  internalAction({
    args: spec.args,
    handler: (ctx: RawActionCtx, args: ObjectType<V>): Promise<A> =>
      run(spec.handler(args).pipe(Effect.provide(Layer.mergeAll(Layer.succeed(ActionCtx, ActionCtx.of(ctx)), Transcriber.layer, Digest.layer))))
  })
