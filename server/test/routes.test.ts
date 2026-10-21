import { createHmac, randomBytes } from "node:crypto"
import { describe, expect, it } from "vitest"
import StripeSdk from "stripe"
import { Effect, Layer, Redacted } from "effect"
import { SseMeter } from "../src/SseMeter.js"
import { AppConfig } from "../src/services/AppConfig.js"
import { type AppleEntitlement, AppStore, entitlementFrom } from "../src/services/AppStore.js"
import { entitlementOfSubscription, Stripe, type SubscriptionState } from "../src/services/Stripe.js"
import { UpstreamFailed, UpstreamUnavailable } from "../src/Errors.js"
import { accountOf, answer, api, get, makeApp, makeConvex, post, SECRET, sse } from "./harness.js"

const bytes = (s: string) => new TextEncoder().encode(s)

describe("SseMeter", () => {
  it("reads the final usage however the bytes are split, CRLF framing and split UTF-8 included", () => {
    const stream = Buffer.concat(answer("Gnade — grâce", { input: 12, output: 34, cacheRead: 56, cacheWrite: 78 }))
    const crlf = bytes(new TextDecoder().decode(stream).replaceAll("\n", "\r\n"))
    for (const whole of [stream, crlf]) {
      for (const size of [1, 3, 7, whole.length]) {
        const meter = new SseMeter(() => 42)
        for (let i = 0; i < whole.length; i += size) meter.push(whole.subarray(i, i + size))
        expect(meter.result()).toEqual({ inputTokens: 12, outputTokens: 34, cacheReadTokens: 56, cacheWriteTokens: 78 })
        expect(meter.stopped).toBe(true)
        expect(meter.firstTokenAt).toBe(42)
      }
    }
  })

  it("has no usage until the final message_delta, and keeps the largest cumulative count", () => {
    const meter = new SseMeter()
    meter.push(sse({ type: "message_start", message: { usage: { input_tokens: 900, output_tokens: 1, cache_read_input_tokens: 5 } } }))
    expect(meter.result()).toBeUndefined()
    meter.push(sse({ type: "message_delta", usage: { output_tokens: 300 } }))
    expect(meter.result()).toEqual({ inputTokens: 900, outputTokens: 300, cacheReadTokens: 5, cacheWriteTokens: 0 })
  })

  it("notices error events and ignores junk", () => {
    const meter = new SseMeter()
    meter.push(bytes(": ping\n\ndata: {not json\n\nevent: error\ndata: {\"type\":\"error\",\"error\":{}}\n\n"))
    expect(meter.errored).toBe(true)
    expect(meter.result()).toBeUndefined()
  })
})

describe("pure policy", () => {
  it("maps Stripe subscription states onto entitlements", () => {
    const sub = (status: string): SubscriptionState => ({ id: "sub_1", customer: "cus_1", status, productId: "price_1", periodStart: 0, periodEnd: Date.UTC(2099, 0, 1), livemode: false })
    expect(entitlementOfSubscription(sub("active")).status).toBe("active")
    expect(entitlementOfSubscription(sub("trialing")).status).toBe("active")
    const pastDue = entitlementOfSubscription(sub("past_due"))
    expect(pastDue).toEqual({ status: "grace", expiresAt: Date.UTC(2099, 0, 15) })
    expect(entitlementOfSubscription(sub("canceled")).expiresAt).toBeLessThanOrEqual(Date.now())
    expect(entitlementOfSubscription(sub("unpaid")).status).toBe("expired")
  })

  it("maps App Store transactions and notifications onto entitlements", () => {
    const now = Date.UTC(2026, 9, 10)
    const tx = { originalTransactionId: "100", productId: "pro.monthly", purchaseDate: Date.UTC(2026, 9, 1), expiresDate: Date.UTC(2026, 10, 1), environment: "Sandbox" } as never
    expect(entitlementFrom(tx, undefined, undefined, now)?.status).toBe("active")
    expect(entitlementFrom({ ...(tx as object), revocationDate: now } as never, undefined, undefined, now)?.status).toBe("revoked")
    expect(entitlementFrom(tx, undefined, { type: "REFUND" }, now)?.status).toBe("revoked")
    expect(entitlementFrom(tx, undefined, { type: "EXPIRED" }, now)?.status).toBe("expired")
    expect(entitlementFrom(tx, undefined, { type: "DID_FAIL_TO_RENEW" }, now)?.status).toBe("billing_retry")
    const lapsed = { ...(tx as object), expiresDate: now - 1 } as never
    const grace = entitlementFrom(lapsed, { gracePeriodExpiresDate: now + 1000 } as never, { type: "DID_FAIL_TO_RENEW", subtype: "GRACE_PERIOD" }, now)
    expect(grace).toMatchObject({ status: "grace", expiresAt: now + 1000 })
    expect(entitlementFrom(lapsed, undefined, undefined, now)?.status).toBe("expired")
    expect(entitlementFrom({ productId: "x" } as never, undefined, undefined, now)).toBeUndefined()
  })
})

/** Fakes for the vendors that record what the routes asked of them. */
const recordingVendors = (opts: { subscription?: () => Effect.Effect<SubscriptionState, UpstreamFailed>; apple?: AppleEntitlement; stripe?: Layer.Layer<Stripe> } = {}) => {
  const calls = { deletedCustomers: [] as Array<string>, subscriptionReads: 0 }
  const unavailable = Effect.fail(new UpstreamUnavailable({ message: "not in tests" }))
  const layer = Layer.mergeAll(
    Layer.succeed(
      AppStore,
      AppStore.of({ verifyTransaction: () => (opts.apple ? Effect.succeed(opts.apple) : unavailable), verifyNotification: () => unavailable, history: () => unavailable })
    ),
    opts.stripe ??
      Layer.succeed(Stripe, Stripe.of({ ensureCustomer: () => unavailable, checkout: () => unavailable, portal: () => unavailable, parseEvent: () => unavailable, subscription: () => unavailable, deleteCustomer: (id) => Effect.sync(() => void calls.deletedCustomers.push(id)) }))
  )
  return { calls, layer }
}

describe("the Stripe webhook", () => {
  it("verifies the signature, upserts the subscription's current state once per event, and retries after a failure", async () => {
    const whsec = `whsec_${randomBytes(16).toString("hex")}`
    let reads = 0
    let fail = true
    const sub: SubscriptionState = { id: "sub_9", customer: "cus_9", status: "active", productId: "price_m", periodStart: Date.UTC(2026, 9, 1), periodEnd: Date.UTC(2099, 0, 1), livemode: false }
    const stripe = Layer.effect(
      Stripe,
      Effect.gen(function*() {
        const real = yield* Stripe
        return Stripe.of({
          ...real,
          subscription: () =>
            Effect.suspend(() => {
              reads++
              return fail ? Effect.fail(new UpstreamFailed({ message: "down" })) : Effect.succeed(sub)
            })
        })
      })
    ).pipe(
      Layer.provide(Stripe.layer),
      Layer.provide(
        AppConfig.layerTest({
          stripe: { secretKey: Redacted.make("sk_test_fake"), webhookSecret: Redacted.make(whsec), priceMonthly: undefined, priceYearly: undefined, returnUrl: "" }
        })
      )
    )
    const t = await makeConvex()
    await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_web" })
    await t.mutation(api.users.setStripeCustomer, { secret: SECRET, clerkId: "user_web", stripeCustomerId: "cus_9" })
    const { handler } = makeApp({ t, vendors: recordingVendors({ stripe }).layer })

    const payload = JSON.stringify({ id: "evt_1", object: "event", type: "customer.subscription.updated", data: { object: { id: "sub_9", object: "subscription" } } })
    const sign = (secret: string) => new StripeSdk("sk_test_fake").webhooks.generateTestHeaderString({ payload, secret })
    const deliver = (signature: string) =>
      handler(new Request("http://proxy.test/v1/webhooks/stripe", { method: "POST", headers: { "stripe-signature": signature }, body: payload }))

    expect((await deliver(sign("whsec_wrong"))).status).toBe(400)
    expect(reads).toBe(0)
    // Stripe can't be reached: 503 so Stripe retries, and the claim is released for that retry.
    expect((await deliver(sign(whsec))).status).toBe(503)
    fail = false
    expect((await deliver(sign(whsec))).status).toBe(200)
    expect((await deliver(sign(whsec))).status).toBe(200)
    expect(reads).toBe(2)
    expect((await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_web" })).plan).toBe("pro")
  })
})

describe("the Clerk webhook", () => {
  it("purges a deleted user's data when Svix's signature checks out", async () => {
    const key = randomBytes(24)
    const secret = `whsec_${key.toString("base64")}`
    const t = await makeConvex()
    await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_leaving" })
    const { handler } = makeApp({ t, config: { clerk: { secretKey: undefined, jwtKey: undefined, issuer: undefined, authorizedParties: [], webhookSecret: Redacted.make(secret) } } })
    const body = JSON.stringify({ type: "user.deleted", object: "event", data: { id: "user_leaving", deleted: true, object: "user" } })
    const deliver = (signWith: Buffer) => {
      const id = `msg_${randomBytes(6).toString("hex")}`
      const ts = Math.floor(Date.now() / 1000).toString()
      const sig = createHmac("sha256", signWith).update(`${id}.${ts}.${body}`).digest("base64")
      return handler(new Request("http://proxy.test/v1/webhooks/clerk", { method: "POST", headers: { "svix-id": id, "svix-timestamp": ts, "svix-signature": `v1,${sig}`, "content-type": "application/json" }, body }))
    }
    expect((await deliver(randomBytes(24))).status).toBe(401)
    expect((await accountOf(t, "user_leaving")).user).not.toBeNull()
    expect((await deliver(key)).status).toBe(200)
    expect((await accountOf(t, "user_leaving")).user).toBeNull()
  })
})

describe("JSON routes", () => {
  it("GET /v1/me creates the account and reports the Free plan", async () => {
    const t = await makeConvex()
    const { handler } = makeApp({ t })
    const res = await handler(get("/v1/me", "user_new"))
    expect(res.status).toBe(200)
    const me = await res.json()
    expect(me.plan).toBe("free")
    expect(me.user.appAccountToken).toMatch(/^[0-9a-f-]{36}$/)
    expect((await handler(new Request("http://proxy.test/v1/me"))).status).toBe(401)
    const bad = await handler(new Request("http://proxy.test/v1/me", { headers: { authorization: "Bearer forged" } }))
    expect(bad.status).toBe(401)
    expect((await bad.json()).error.type).toBe("unauthenticated")
  })

  it("accepts an App Store purchase only for the account it was bought for", async () => {
    const t = await makeConvex()
    const me = await t.mutation(api.users.me, { secret: SECRET, clerkId: "user_ios" })
    const purchase = (appAccountToken: string): AppleEntitlement => ({ originalId: "77", productId: "pro.yearly", appAccountToken, status: "active", periodStart: Date.now(), expiresAt: Date.UTC(2099, 0, 1), environment: "Sandbox" })
    const other = makeApp({ t, vendors: recordingVendors({ apple: purchase(crypto.randomUUID()) }).layer })
    const res = await other.handler(post("/v1/billing/apple/transactions", { jws: "x.y.z" }, "user_ios"))
    expect(res.status).toBe(400)
    const mine = makeApp({ t, vendors: recordingVendors({ apple: purchase(me.user.appAccountToken.toUpperCase()) }).layer })
    const ok = await mine.handler(post("/v1/billing/apple/transactions", { jws: "x.y.z" }, "user_ios"))
    expect(ok.status).toBe(200)
    expect((await ok.json()).plan).toBe("pro")
  })

  it("DELETE /v1/account removes the user's rows and their Stripe customer", async () => {
    const t = await makeConvex()
    const vendors = recordingVendors()
    const { handler } = makeApp({ t, vendors: vendors.layer })
    expect((await handler(get("/v1/me", "user_bye"))).status).toBe(200)
    await t.mutation(api.users.setStripeCustomer, { secret: SECRET, clerkId: "user_bye", stripeCustomerId: "cus_bye" })
    const res = await handler(new Request("http://proxy.test/v1/account", { method: "DELETE", headers: { authorization: "Bearer test:user_bye" } }))
    expect(res.status).toBe(204)
    expect(vendors.calls.deletedCustomers).toEqual(["cus_bye"])
    expect((await accountOf(t, "user_bye")).user).toBeNull()
  })

  it("keeps the internal routes to holders of the server secret", async () => {
    const t = await makeConvex()
    const vendors = recordingVendors()
    const { handler } = makeApp({ t, vendors: vendors.layer })
    const call = (token: string) =>
      handler(new Request("http://proxy.test/v1/internal/apple/reconcile", { method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" }, body: JSON.stringify({ sinceMs: 0 }) }))
    expect((await call("test-secre")).status).toBe(401)
    expect((await call("test:user_x")).status).toBe(401)
    const res = await call(SECRET)
    expect(res.status).toBe(200)
    expect(await res.json()).toEqual({ replayed: 0, skipped: "The App Store isn't configured." })
  })

  it("answers the retired /v1/sermons routes 409 contract_unsupported, so a build from before M1b says to update", async () => {
    const t = await makeConvex()
    const { handler } = makeApp({ t })
    for (const req of [post("/v1/sermons", { recordingId: "r", durationSec: 60, bytes: 1_920_044, contentType: "audio/wav" }), post("/v1/sermons/s1/transcribe", {}), get("/v1/sermons/s1")]) {
      const res = await handler(req)
      expect(res.status, req.url).toBe(409)
      expect((await res.json()).error).toMatchObject({ type: "contract_unsupported", message: "Update Linger to transcribe sermons." })
    }
  })

  it("answers /v1/health without authentication", async () => {
    const t = await makeConvex()
    const { handler } = makeApp({ t })
    expect((await handler(new Request("http://proxy.test/v1/health"))).status).toBe(200)
  })
})
