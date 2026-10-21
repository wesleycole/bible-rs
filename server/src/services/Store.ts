/**
 * Convex, as the proxy sees it: every mutation and query it calls, with the server secret added.
 * The live layer talks to the deployment over HTTP; tests run the same Convex functions in-process
 * through `convex-test`, so allowance logic is exercised for real.
 */
import { ConvexHttpClient } from "convex/browser"
import type { FunctionArgs, FunctionReference, FunctionReturnType } from "convex/server"
import { Context, Effect, Layer, Redacted, Schedule } from "effect"
import { api } from "../../convex/_generated/api.js"
import { UpstreamUnavailable } from "../Errors.js"
import { AppConfig } from "./AppConfig.js"

type Ref = FunctionReference<"query" | "mutation" | "action", "public">

/** Calls Convex functions; the secret argument is added by `Store`. */
export interface ConvexCaller {
  readonly query: (ref: FunctionReference<"query", "public">, args: Record<string, unknown>) => Promise<unknown>
  readonly mutation: (ref: FunctionReference<"mutation", "public">, args: Record<string, unknown>) => Promise<unknown>
  readonly action: (ref: FunctionReference<"action", "public">, args: Record<string, unknown>) => Promise<unknown>
}

/** A function's arguments without the server secret, which `Store` supplies. */
type Args<F extends Ref> = Omit<FunctionArgs<F>, "secret">
type Result<F extends Ref> = Effect.Effect<FunctionReturnType<F>, UpstreamUnavailable>

const fns = {
  beginRound: api.usage.beginRound,
  settleRound: api.usage.settleRound,
  me: api.users.me,
  setStripeCustomer: api.users.setStripeCustomer,
  billingProfile: api.users.billingProfile,
  upsertEntitlement: api.entitlements.upsert,
  claimWebhook: api.webhooks.claim,
  releaseWebhook: api.webhooks.release,
  purgeAccount: api.account.purge
} as const

type Fns = typeof fns
export type StoreService = { readonly [K in keyof Fns]: (args: Args<Fns[K]>) => Result<Fns[K]> }

const kindOf: { readonly [K in keyof Fns]: "query" | "mutation" | "action" } = {
  beginRound: "mutation",
  settleRound: "mutation",
  me: "mutation",
  setStripeCustomer: "mutation",
  billingProfile: "query",
  upsertEntitlement: "mutation",
  claimWebhook: "mutation",
  releaseWebhook: "mutation",
  purgeAccount: "mutation"
}

const make = (caller: ConvexCaller, secret: string): StoreService => {
  const entries = (Object.keys(fns) as Array<keyof Fns>).map((name) => {
    const kind = kindOf[name]
    const ref = fns[name] as never
    const call = (args: Record<string, unknown>) =>
      Effect.tryPromise({
        try: () => caller[kind](ref, { ...args, secret }),
        catch: (cause) => cause
      }).pipe(
        // Only queries retry: a mutation whose reply was lost may have committed, and running
        // beginRound twice would hold a second reservation.
        Effect.retry({ times: kind === "query" ? 2 : 0, schedule: Schedule.exponential("100 millis") }),
        Effect.tapError((cause) => Effect.logError(`Convex ${name} failed`, cause)),
        Effect.mapError(() => new UpstreamUnavailable({ message: "Linger's servers are unavailable right now." })),
        Effect.withSpan(`store.${name}`)
      )
    return [name, call] as const
  })
  return Object.fromEntries(entries) as unknown as StoreService
}

export class Store extends Context.Service<Store, StoreService>()("linger/services/Store") {
  static readonly layer = Layer.effect(
    Store,
    Effect.gen(function*() {
      const config = yield* AppConfig
      const client = new ConvexHttpClient(config.convexUrl)
      const caller: ConvexCaller = {
        query: (ref, args) => client.query(ref, args as never),
        // The client queues mutations one at a time by default, which would line every user's
        // requests on this instance up behind each other. Each request's calls are already ordered.
        mutation: (ref, args) => client.mutation(ref, args as never, { skipQueue: true }),
        action: (ref, args) => client.action(ref, args as never)
      }
      return Store.of(make(caller, Redacted.value(config.serverSecret)))
    })
  )

  /** Any caller (a `convex-test` instance in tests). */
  static readonly fromCaller = (caller: ConvexCaller, secret: string) => Layer.succeed(Store, Store.of(make(caller, secret)))
}
