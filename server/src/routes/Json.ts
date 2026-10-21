/**
 * Handlers for the HttpApi groups in Api.ts: the bearer-token middleware, GET /v1/me, billing and
 * account deletion.
 */
import { Effect, Layer, Redacted } from "effect"
import { HttpApiBuilder } from "effect/http-api"
import { Api, Authorization, type Me } from "../Api.js"
import { InvalidRequest, Unauthenticated } from "../Errors.js"
import { AppStore } from "../services/AppStore.js"
import { Auth, CurrentUserRef } from "../services/Auth.js"
import { Store } from "../services/Store.js"
import { Stripe } from "../services/Stripe.js"

export const AuthorizationLive = Layer.effect(
  Authorization,
  Effect.gen(function*() {
    const auth = yield* Auth
    return Authorization.of({
      bearer: Effect.fn(function*(httpEffect, { credential }) {
        const token = Redacted.value(credential)
        if (!token) return yield* new Unauthenticated({ message: "Sign in to use this." })
        const user = yield* auth.verify(token)
        return yield* Effect.provideService(httpEffect, CurrentUserRef, user)
      })
    })
  })
)

export const MeHandlers = HttpApiBuilder.group(
  Api,
  "me",
  Effect.fn(function*(handlers) {
    const store = yield* Store
    return handlers.handle("me", () =>
      Effect.gen(function*() {
        const user = yield* CurrentUserRef
        return (yield* store.me({ clerkId: user.clerkId, email: user.email })) as Me
      }))
  })
)

export const BillingHandlers = HttpApiBuilder.group(
  Api,
  "billing",
  Effect.fn(function*(handlers) {
    const store = yield* Store
    const appStore = yield* AppStore
    const stripe = yield* Stripe

    return handlers
      .handle("appleTransaction", ({ payload }) =>
        Effect.gen(function*() {
          const user = yield* CurrentUserRef
          const me = (yield* store.me({ clerkId: user.clerkId, email: user.email })) as Me
          const e = yield* appStore.verifyTransaction(payload.jws)
          // A purchase made for another Linger account (shared Apple ID) isn't this user's.
          if (e.appAccountToken && e.appAccountToken.toLowerCase() !== me.user.appAccountToken.toLowerCase()) {
            return yield* new InvalidRequest({ message: "That purchase belongs to another Linger account." })
          }
          yield* store.upsertEntitlement({
            owner: { clerkId: user.clerkId },
            source: "app_store",
            productId: e.productId,
            status: e.status,
            periodStart: e.periodStart,
            expiresAt: e.expiresAt,
            originalId: e.originalId,
            environment: e.environment
          })
          return (yield* store.me({ clerkId: user.clerkId, email: user.email })) as Me
        }))
      .handle("stripeCheckout", ({ payload }) =>
        Effect.gen(function*() {
          const user = yield* CurrentUserRef
          yield* store.me({ clerkId: user.clerkId, email: user.email })
          const profile = yield* store.billingProfile({ clerkId: user.clerkId })
          if (!profile) return yield* new InvalidRequest({ message: "No such user." })
          const customer = yield* stripe.ensureCustomer({
            clerkId: user.clerkId,
            email: profile.email,
            existing: profile.stripeCustomerId
          })
          if (customer !== profile.stripeCustomerId) {
            yield* store.setStripeCustomer({ clerkId: user.clerkId, stripeCustomerId: customer })
          }
          return { url: yield* stripe.checkout({ customer, interval: payload.interval, clientReferenceId: profile.userId }) }
        }))
      .handle("stripePortal", () =>
        Effect.gen(function*() {
          const user = yield* CurrentUserRef
          const profile = yield* store.billingProfile({ clerkId: user.clerkId })
          if (!profile?.stripeCustomerId) {
            return yield* new InvalidRequest({ message: "There's no subscription bought on the web to manage." })
          }
          return { url: yield* stripe.portal(profile.stripeCustomerId) }
        }))
  })
)

/** Deletes what a purged account left with vendors (its Stripe customer). Best effort: the service logs its own failure. */
export const cleanupVendors = Effect.fn("account.cleanup")(function*(left: { readonly stripeCustomerId?: string | undefined }) {
  if (left.stripeCustomerId) yield* (yield* Stripe).deleteCustomer(left.stripeCustomerId)
})

export const AccountHandlers = HttpApiBuilder.group(
  Api,
  "account",
  Effect.fn(function*(handlers) {
    const store = yield* Store
    const auth = yield* Auth
    const stripe = yield* Stripe
    return handlers.handle("delete", () =>
      Effect.gen(function*() {
        const user = yield* CurrentUserRef
        const left = yield* store.purgeAccount({ clerkId: user.clerkId })
        yield* cleanupVendors(left).pipe(Effect.provideService(Stripe, stripe))
        yield* auth.deleteUser(user.clerkId)
      }))
  })
)

export const JsonRoutes = HttpApiBuilder.layer(Api).pipe(
  Layer.provide([MeHandlers, BillingHandlers, AccountHandlers]),
  Layer.provide(AuthorizationLive)
)
