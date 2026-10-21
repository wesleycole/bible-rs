/**
 * /v1/webhooks/{apple,stripe,clerk}. Each verifies its sender on the raw body, is made
 * idempotent through Convex's webhookEvents table, and answers 500 on failure so the sender
 * retries (the claim is released first).
 */
import { verifyWebhook } from "@clerk/backend/webhooks"
import { Effect, Layer, Redacted, Schema } from "effect"
import { HttpRouter, HttpServerRequest, HttpServerResponse } from "effect/http"
import { type ApiError, InvalidRequest, toResponse, Unauthenticated, UpstreamUnavailable } from "../Errors.js"
import { AppConfig } from "../services/AppConfig.js"
import { type AppleNotification, AppStore } from "../services/AppStore.js"
import { Store } from "../services/Store.js"
import { entitlementOfSubscription, Stripe } from "../services/Stripe.js"
import { cleanupVendors } from "./Json.js"

const ok = HttpServerResponse.jsonUnsafe({ received: true })

/** Runs a handler once per event id; a failed handler releases its claim so a retry runs it again. */
const once = <E, R>(source: string, eventId: string, handle: Effect.Effect<void, E, R>) =>
  Effect.gen(function*() {
    const store = yield* Store
    const { fresh } = yield* store.claimWebhook({ source, eventId })
    if (!fresh) return ok
    yield* handle.pipe(
      Effect.tapError(() => store.releaseWebhook({ source, eventId }).pipe(Effect.ignore))
    )
    return ok
  }).pipe(Effect.withSpan(`webhook.${source}`))

const respond = <R>(effect: Effect.Effect<HttpServerResponse.HttpServerResponse, unknown, R>) =>
  effect.pipe(
    Effect.catch((e: unknown) => {
      const tag = (e as { _tag?: string })._tag
      return tag === "InvalidRequest" || tag === "Unauthenticated"
        ? Effect.succeed(toResponse(e as ApiError))
        : Effect.logError("webhook failed", e).pipe(Effect.as(toResponse(new UpstreamUnavailable({ message: "Try again." }))))
    })
  )

/** Upserts an App Store entitlement from a verified notification (webhook or nightly replay). */
export const applyAppleNotification = (n: AppleNotification) =>
  once(
    "apple",
    n.notificationId,
    Effect.gen(function*() {
      const store = yield* Store
      const e = n.entitlement
      if (!e) return
      const result = yield* store.upsertEntitlement({
        owner: e.appAccountToken ? { appAccountToken: e.appAccountToken } : undefined,
        source: "app_store",
        productId: e.productId,
        status: e.status,
        periodStart: e.periodStart,
        expiresAt: e.expiresAt,
        originalId: e.originalId,
        environment: e.environment
      })
      if (!result.ok) yield* Effect.logWarning("App Store notification for an unknown user", { type: n.type })
    })
  )

const AppleHook = Schema.Struct({ signedPayload: Schema.String })

export const WebhookRoutes = HttpRouter.use((router) =>
  Effect.gen(function*() {
    const config = yield* AppConfig
    const services = Layer.mergeAll(
      Layer.succeed(Store, yield* Store),
      Layer.succeed(Stripe, yield* Stripe),
      Layer.succeed(AppStore, yield* AppStore)
    )
    const route = (path: HttpRouter.PathInput, handler: (request: HttpServerRequest.HttpServerRequest, body: string) => Effect.Effect<HttpServerResponse.HttpServerResponse, unknown, Store | Stripe | AppStore>) =>
      router.add("POST", path, (request) =>
        respond(
          request.text.pipe(
            Effect.mapError(() => new InvalidRequest({ message: "Unreadable body." })),
            Effect.flatMap((body) => handler(request, body)),
            Effect.provide(services)
          )
        ))

    const json = <S extends Schema.Top>(schema: S, body: string) =>
      Effect.try({ try: () => JSON.parse(body) as unknown, catch: () => new InvalidRequest({ message: "Not JSON." }) }).pipe(
        Effect.flatMap(Schema.decodeUnknownEffect(schema)),
        Effect.mapError(() => new InvalidRequest({ message: "Unexpected payload." }))
      )

    yield* route("/v1/webhooks/stripe", (request, body) =>
      Effect.gen(function*() {
        const stripe = yield* Stripe
        const event = yield* stripe.parseEvent(body, request.headers["stripe-signature"] ?? "")
        const handled = [
          "checkout.session.completed",
          "customer.subscription.created",
          "customer.subscription.updated",
          "customer.subscription.deleted",
          "invoice.payment_failed",
          "invoice.paid"
        ]
        if (!handled.includes(event.type) || !event.subscriptionId) return ok
        const subscriptionId = event.subscriptionId
        return yield* once(
          "stripe",
          event.id,
          Effect.gen(function*() {
            // The subscription's current state, not the event's snapshot: events arrive out of order.
            const sub = yield* stripe.subscription(subscriptionId)
            const { status, expiresAt } = entitlementOfSubscription(sub)
            const result = yield* (yield* Store).upsertEntitlement({
              owner: { stripeCustomerId: sub.customer },
              source: "stripe",
              productId: sub.productId,
              status,
              periodStart: sub.periodStart,
              expiresAt,
              originalId: sub.id,
              environment: sub.livemode ? "production" : "test"
            })
            if (!result.ok) yield* Effect.logWarning("Stripe subscription for an unknown customer")
          })
        )
      }))

    yield* route("/v1/webhooks/apple", (_request, body) =>
      Effect.gen(function*() {
        const hook = yield* json(AppleHook, body)
        const notification = yield* (yield* AppStore).verifyNotification(hook.signedPayload)
        return yield* applyAppleNotification(notification)
      }))

    yield* route("/v1/webhooks/clerk", (request, body) =>
      Effect.gen(function*() {
        const secret = config.clerk.webhookSecret
        if (!secret) return yield* new UpstreamUnavailable({ message: "Clerk webhooks aren't configured." })
        const event = yield* Effect.tryPromise({
          try: () =>
            verifyWebhook(
              new Request(`${config.publicUrl}/v1/webhooks/clerk`, {
                method: "POST",
                headers: request.headers as Record<string, string>,
                body
              }),
              { signingSecret: Redacted.value(secret) }
            ),
          catch: () => new Unauthenticated({ message: "Bad Clerk signature." })
        })
        if (event.type !== "user.deleted" || !event.data.id) return ok
        const clerkId = event.data.id
        return yield* once(
          "clerk",
          request.headers["svix-id"] ?? `user.deleted:${clerkId}`,
          Effect.gen(function*() {
            const left = yield* (yield* Store).purgeAccount({ clerkId })
            yield* cleanupVendors(left)
          })
        )
      }))
  })
)
