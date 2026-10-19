/**
 * Stripe, for the Developer ID Mac app and Linux: Checkout for the two Pro prices, the Customer
 * Portal, and subscription webhooks verified on the raw body.
 */
import StripeSdk from "stripe"
import { Context, Effect, Layer, Redacted } from "effect"
import { InvalidRequest, UpstreamFailed, UpstreamUnavailable } from "../Errors.js"
import { AppConfig } from "./AppConfig.js"
import type { EntitlementStatus } from "./AppStore.js"

export interface SubscriptionState {
  readonly id: string
  readonly customer: string
  readonly status: string
  readonly productId: string
  readonly periodStart: number
  readonly periodEnd: number
  readonly livemode: boolean
}

/** Stripe retries a failed renewal for up to about two weeks; Pro stays on meanwhile. */
const PAST_DUE_GRACE_MS = 14 * 24 * 60 * 60 * 1000

/** A Stripe subscription as an entitlement row's status and expiry. */
export const entitlementOfSubscription = (
  s: SubscriptionState
): { readonly status: EntitlementStatus; readonly expiresAt: number } => {
  switch (s.status) {
    case "active":
    case "trialing":
      return { status: "active", expiresAt: s.periodEnd }
    case "past_due":
      return { status: "grace", expiresAt: s.periodEnd + PAST_DUE_GRACE_MS }
    case "canceled":
      return { status: "canceled", expiresAt: Math.min(s.periodEnd, Date.now()) }
    case "incomplete":
      return { status: "billing_retry", expiresAt: s.periodEnd }
    default:
      return { status: "expired", expiresAt: s.periodEnd }
  }
}

export interface StripeEvent {
  readonly id: string
  readonly type: string
  /** The subscription the event concerns, when it concerns one. */
  readonly subscriptionId: string | undefined
}

export interface StripeService {
  readonly ensureCustomer: (input: {
    readonly clerkId: string
    readonly email: string | undefined
    readonly existing: string | undefined
  }) => Effect.Effect<string, UpstreamFailed | UpstreamUnavailable>
  readonly checkout: (input: {
    readonly customer: string
    readonly interval: "monthly" | "yearly"
    readonly clientReferenceId: string
  }) => Effect.Effect<string, UpstreamFailed | UpstreamUnavailable>
  readonly portal: (customer: string) => Effect.Effect<string, UpstreamFailed | UpstreamUnavailable>
  readonly parseEvent: (rawBody: string, signature: string) => Effect.Effect<StripeEvent, InvalidRequest | UpstreamUnavailable>
  readonly subscription: (id: string) => Effect.Effect<SubscriptionState, UpstreamFailed | UpstreamUnavailable>
  /** Deletes the customer, which cancels their subscriptions (account deletion). */
  readonly deleteCustomer: (id: string) => Effect.Effect<void>
}

const unconfigured = new UpstreamUnavailable({ message: "Payments aren't configured on this server." })

export class Stripe extends Context.Service<Stripe, StripeService>()("linger/services/Stripe") {
  static readonly layer = Layer.effect(
    Stripe,
    Effect.gen(function*() {
      const { stripe: cfg } = yield* AppConfig
      if (!cfg.secretKey) {
        return Stripe.of({
          ensureCustomer: () => Effect.fail(unconfigured),
          checkout: () => Effect.fail(unconfigured),
          portal: () => Effect.fail(unconfigured),
          parseEvent: () => Effect.fail(unconfigured),
          subscription: () => Effect.fail(unconfigured),
          deleteCustomer: () => Effect.void
        })
      }
      const sdk = new StripeSdk(Redacted.value(cfg.secretKey))
      const call = <A>(what: string, f: () => Promise<A>) =>
        Effect.tryPromise(f).pipe(
          Effect.tapError((cause) => Effect.logError(`Stripe ${what} failed`, cause)),
          Effect.mapError(() => new UpstreamFailed({ message: "Payments are unavailable right now." })),
          Effect.withSpan(`stripe.${what}`)
        )

      const ensureCustomer: StripeService["ensureCustomer"] = ({ clerkId, email, existing }) =>
        existing
          ? Effect.succeed(existing)
          : call("createCustomer", () =>
            sdk.customers.create({ ...(email ? { email } : {}), metadata: { clerkId } }, { idempotencyKey: `customer:${clerkId}` })
          ).pipe(Effect.map((c) => c.id))

      const checkout: StripeService["checkout"] = ({ customer, interval, clientReferenceId }) => {
        const price = interval === "monthly" ? cfg.priceMonthly : cfg.priceYearly
        if (!price) return Effect.fail(unconfigured)
        return call("checkout", () =>
          sdk.checkout.sessions.create({
            mode: "subscription",
            customer,
            client_reference_id: clientReferenceId,
            line_items: [{ price, quantity: 1 }],
            success_url: `${cfg.returnUrl}?status=success`,
            cancel_url: `${cfg.returnUrl}?status=cancel`,
            allow_promotion_codes: true
          })
        ).pipe(Effect.flatMap((s) => (s.url ? Effect.succeed(s.url) : Effect.fail(new UpstreamFailed({ message: "Stripe returned no checkout URL." })))))
      }

      const portal: StripeService["portal"] = (customer) =>
        call("portal", () => sdk.billingPortal.sessions.create({ customer, return_url: cfg.returnUrl })).pipe(
          Effect.map((s) => s.url)
        )

      const parseEvent: StripeService["parseEvent"] = (rawBody, signature) => {
        if (!cfg.webhookSecret) return Effect.fail(unconfigured)
        const secret = Redacted.value(cfg.webhookSecret)
        return Effect.try({
          try: () => sdk.webhooks.constructEvent(rawBody, signature, secret),
          catch: () => new InvalidRequest({ message: "Bad Stripe signature." })
        }).pipe(Effect.map((event) => ({ id: event.id, type: event.type, subscriptionId: subscriptionIdOf(event.data.object) })))
      }

      const subscription: StripeService["subscription"] = (id) =>
        call("retrieveSubscription", () => sdk.subscriptions.retrieve(id)).pipe(
          Effect.map((s) => {
            const item = s.items.data[0]
            return {
              id: s.id,
              customer: typeof s.customer === "string" ? s.customer : s.customer.id,
              status: s.status,
              productId: item?.price.id ?? "unknown",
              periodStart: (item?.current_period_start ?? s.start_date) * 1000,
              periodEnd: (item?.current_period_end ?? s.start_date) * 1000,
              livemode: s.livemode
            }
          })
        )

      const deleteCustomer: StripeService["deleteCustomer"] = (id) =>
        call("deleteCustomer", () => sdk.customers.del(id)).pipe(Effect.asVoid, Effect.ignore)

      return Stripe.of({ ensureCustomer, checkout, portal, parseEvent, subscription, deleteCustomer })
    })
  )
}

/** The subscription an event's object refers to: a subscription, a checkout session or an invoice. */
export const subscriptionIdOf = (object: unknown): string | undefined => {
  const o = object as {
    object?: string
    id?: string
    subscription?: string | { id: string } | null
    parent?: { subscription_details?: { subscription?: string | { id: string } | null } | null } | null
  }
  if (o.object === "subscription") return o.id
  const ref = o.subscription ?? o.parent?.subscription_details?.subscription
  return typeof ref === "string" ? ref : ref?.id
}
