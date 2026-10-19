import { createHmac, randomBytes } from "node:crypto"
import { afterEach, describe, expect, it, vi } from "vitest"
import StripeSdk from "stripe"
import { Effect, Layer, Redacted } from "effect"
import { SseMeter } from "../src/SseMeter.js"
import { reachable, wavSizeAgrees } from "../src/routes/Json.js"
import { AppConfig } from "../src/services/AppConfig.js"
import { type AppleEntitlement, AppStore, entitlementFrom } from "../src/services/AppStore.js"
import { Blob } from "../src/services/Blob.js"
import { entitlementOfSubscription, Stripe, type SubscriptionState } from "../src/services/Stripe.js"
import { type Job, Transcriber } from "../src/services/Transcriber.js"
import { UpstreamFailed, UpstreamUnavailable } from "../src/Errors.js"
import { accountOf, answer, api, get, makeApp, makeConvex, post, SECRET, sse, type TestConvex } from "./harness.js"

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
  it("accepts a WAV size only near its duration's 16 kHz mono size", () => {
    expect(wavSizeAgrees(3600, 3600 * 32000 + 44)).toBe(true)
    expect(wavSizeAgrees(3600, Math.round(3600 * 32000 * 0.985))).toBe(true)
    expect(wavSizeAgrees(3600, Math.round(3600 * 32000 * 0.97))).toBe(false)
    // A 10-minute claim on a 3-hour file.
    expect(wavSizeAgrees(600, 3 * 3600 * 32000)).toBe(false)
    expect(wavSizeAgrees(1, 32044 + 60 * 1024)).toBe(true)
  })

  it("only calls back URLs the internet can reach", () => {
    for (const url of ["http://127.0.0.1:3000", "http://localhost:3000", "http://10.1.2.3", "http://192.168.1.4", "http://172.20.0.1", "http://[::1]:3000", "http://mac.local"]) {
      expect(reachable(url), url).toBe(false)
    }
    for (const url of ["https://api.linger.app", "http://172.32.0.1", "https://x.onamp.dev"]) expect(reachable(url), url).toBe(true)
  })

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
const recordingVendors = (opts: { job?: () => Job; subscription?: () => Effect.Effect<SubscriptionState, UpstreamFailed>; apple?: AppleEntitlement; stripe?: Layer.Layer<Stripe> } = {}) => {
  const calls = { started: [] as Array<{ webhook?: { url: string; secret: string } }>, deletedBlobs: [] as Array<string>, deletedJobs: [] as Array<string>, subscriptionReads: 0, uploads: new Map<string, number>() }
  const unavailable = Effect.fail(new UpstreamUnavailable({ message: "not in tests" }))
  const layer = Layer.mergeAll(
    Layer.succeed(
      Blob,
      Blob.of({
        uploadTarget: (path, o) => Effect.succeed({ url: `http://blob.test/${path}`, method: "PUT" as const, headers: { "content-type": o.contentType }, expiresAt: Date.now() + o.validForMs }),
        size: (path) => Effect.succeed(calls.uploads.get(path)),
        transcriberSource: (path) => Effect.succeed({ _tag: "Url" as const, url: `http://blob.test/${path}?signed` }),
        delete: (paths) => Effect.sync(() => void calls.deletedBlobs.push(...paths))
      })
    ),
    Layer.succeed(
      Transcriber,
      Transcriber.of({
        start: (input) => Effect.sync(() => (calls.started.push({ ...(input.webhook ? { webhook: input.webhook } : {}) }), `job_${calls.started.length}`)),
        get: (id) => Effect.succeed({ ...(opts.job?.() ?? { status: "processing" as const }), id }),
        delete: (id) => Effect.sync(() => void calls.deletedJobs.push(id))
      })
    ),
    Layer.succeed(
      AppStore,
      AppStore.of({ verifyTransaction: () => (opts.apple ? Effect.succeed(opts.apple) : unavailable), verifyNotification: () => unavailable, history: () => unavailable })
    ),
    opts.stripe ??
      Layer.succeed(Stripe, Stripe.of({ ensureCustomer: () => unavailable, checkout: () => unavailable, portal: () => unavailable, parseEvent: () => unavailable, subscription: () => unavailable, deleteCustomer: () => Effect.void }))
  )
  return { calls, layer }
}

const completed = (): Job => ({
  id: "",
  status: "completed",
  audio_duration: 61.5,
  language_code: "en",
  utterances: [
    { start: 0, end: 4000, speaker: "A", text: " Turn with me to Romans 8. " },
    { start: 4000, end: 9000, speaker: "B", text: "Amen." }
  ]
})

/** Creates a 60-second sermon over HTTP and marks its upload complete. */
const uploadedSermon = async (handler: (r: Request) => Promise<Response>, calls: ReturnType<typeof recordingVendors>["calls"], user: string) => {
  const res = await handler(post("/v1/sermons", { recordingId: "rec-1", durationSec: 60, bytes: 60 * 32000 + 44, contentType: "audio/wav" }, user))
  expect(res.status).toBe(200)
  const created = await res.json()
  const path = new URL(created.upload.url).pathname.slice(1)
  calls.uploads.set(path, 60 * 32000 + 44)
  return { created, path }
}

afterEach(() => {
  vi.unstubAllGlobals()
})

/** Serves convex-test's storage URLs, which the GET route fetches the transcript from. */
const serveStorage = (t: TestConvex) =>
  vi.stubGlobal("fetch", async (url: string | URL) => {
    // convex-test names a file by its sha256 in the URLs it hands out.
    const sha256 = String(url).split("/api/storage/")[1]!
    const text = await t.run(async (ctx) => {
      const file = (await ctx.db.system.query("_storage").collect()).find((f) => f.sha256 === sha256)!
      return (await ctx.storage.get(file._id))!.text()
    })
    return new Response(text)
  })

describe("sermon routes and the AssemblyAI webhook", () => {
  it("runs a sermon from upload to transcript through the webhook, then leaves nothing with vendors", async () => {
    const t = await makeConvex()
    const vendors = recordingVendors({ job: completed })
    const { handler } = makeApp({ t, vendors: vendors.layer })
    const { created, path } = await uploadedSermon(handler, vendors.calls, "user_s1")
    expect(created.trial).toBe(true)

    const started = await handler(post(`/v1/sermons/${created.sermonId}/transcribe`, {}, "user_s1"))
    expect(await started.json()).toEqual({ sermonId: created.sermonId, status: "transcribing" })
    const webhook = vendors.calls.started[0]!.webhook!
    expect(webhook.url).toBe("http://proxy.test/v1/webhooks/assemblyai")

    const hook = (secret: string) =>
      handler(new Request(webhook.url, { method: "POST", headers: { "x-linger-webhook": secret }, body: JSON.stringify({ transcript_id: "job_1", status: "completed" }) }))
    expect((await hook("not-the-secret")).status).toBe(401)
    expect(vendors.calls.deletedJobs).toEqual([])
    expect((await hook(webhook.secret)).status).toBe(200)
    expect(vendors.calls.deletedJobs).toEqual(["job_1"])
    expect(vendors.calls.deletedBlobs).toEqual([path])

    serveStorage(t)
    const done = await (await handler(get(`/v1/sermons/${created.sermonId}`, "user_s1"))).json()
    expect(done).toEqual({
      sermonId: created.sermonId,
      status: "done",
      transcript: {
        language: "en",
        durationMs: 61500,
        segments: [
          { start_ms: 0, end_ms: 4000, speaker: "A", text: "Turn with me to Romans 8." },
          { start_ms: 4000, end_ms: 9000, speaker: "B", text: "Amen." }
        ]
      }
    })
    const { ledger } = await accountOf(t, "user_s1")
    expect(ledger).toMatchObject([{ kind: "sermon", status: "trial", audioSeconds: 62 }])
  })

  it("without a reachable public URL, finishes the job when the client polls", async () => {
    const t = await makeConvex()
    let status: Job["status"] = "processing"
    const vendors = recordingVendors({ job: () => (status === "completed" ? completed() : { id: "", status }) })
    const { handler } = makeApp({ t, vendors: vendors.layer, config: { publicUrl: "http://127.0.0.1:3000" } })
    const { created } = await uploadedSermon(handler, vendors.calls, "user_s2")
    await handler(post(`/v1/sermons/${created.sermonId}/transcribe`, {}, "user_s2"))
    expect(vendors.calls.started[0]!.webhook).toBeUndefined()
    expect((await (await handler(get(`/v1/sermons/${created.sermonId}`, "user_s2"))).json()).status).toBe("transcribing")
    status = "completed"
    serveStorage(t)
    expect((await (await handler(get(`/v1/sermons/${created.sermonId}`, "user_s2"))).json()).status).toBe("done")
    expect(vendors.calls.deletedJobs).toEqual(["job_1"])
  })

  it("refuses a size that doesn't match the duration, an incomplete upload, and another user's sermon", async () => {
    const t = await makeConvex()
    const vendors = recordingVendors()
    const { handler } = makeApp({ t, vendors: vendors.layer })
    const lie = await handler(post("/v1/sermons", { recordingId: "r", durationSec: 60, bytes: 10 * 3600 * 32000, contentType: "audio/wav" }, "user_s3"))
    expect(lie.status).toBe(400)
    const res = await handler(post("/v1/sermons", { recordingId: "r", durationSec: 60, bytes: 60 * 32000 + 44, contentType: "audio/wav" }, "user_s3"))
    const created = await res.json()
    vendors.calls.uploads.set(new URL(created.upload.url).pathname.slice(1), 1000)
    const partial = await handler(post(`/v1/sermons/${created.sermonId}/transcribe`, {}, "user_s3"))
    expect(partial.status).toBe(400)
    expect((await partial.json()).error.message).toContain("1000 of")
    expect(vendors.calls.started).toEqual([])
    expect((await handler(get(`/v1/sermons/${created.sermonId}`, "user_other"))).status).toBe(404)
  })
})

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

  it("DELETE /v1/account removes the user's rows and their vendor leftovers", async () => {
    const t = await makeConvex()
    const vendors = recordingVendors()
    const { handler } = makeApp({ t, vendors: vendors.layer })
    const { path } = await uploadedSermon(handler, vendors.calls, "user_bye")
    const res = await handler(new Request("http://proxy.test/v1/account", { method: "DELETE", headers: { authorization: "Bearer test:user_bye" } }))
    expect(res.status).toBe(204)
    expect(vendors.calls.deletedBlobs).toEqual([path])
    expect((await accountOf(t, "user_bye")).user).toBeNull()
  })

  it("keeps the internal routes to holders of the server secret", async () => {
    const t = await makeConvex()
    const vendors = recordingVendors()
    const { handler } = makeApp({ t, vendors: vendors.layer })
    const call = (token: string) =>
      handler(new Request("http://proxy.test/v1/internal/cleanup", { method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json" }, body: JSON.stringify({ blobPaths: ["sermons/a"], assemblyIds: ["j1"] }) }))
    expect((await call("test-secre")).status).toBe(401)
    expect((await call("test:user_x")).status).toBe(401)
    expect(vendors.calls.deletedBlobs).toEqual([])
    expect((await call(SECRET)).status).toBe(200)
    expect(vendors.calls).toMatchObject({ deletedBlobs: ["sermons/a"], deletedJobs: ["j1"] })
  })

  it("answers /v1/health without authentication", async () => {
    const t = await makeConvex()
    const { handler } = makeApp({ t })
    expect((await handler(new Request("http://proxy.test/v1/health"))).status).toBe(200)
  })
})
