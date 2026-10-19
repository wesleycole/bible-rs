/**
 * The proxy wired for tests: the real routes and HTTP stack, the real Convex functions running
 * in-process through convex-test (so allowances and settlement are exercised for real), and fakes
 * for the vendors.
 */
import rateLimiter from "@convex-dev/rate-limiter/test"
import { convexTest } from "convex-test"
import { Effect, Layer, Stream } from "effect"
import { HttpRouter, HttpServer } from "effect/http"
import { api, internal } from "../convex/_generated/api.js"
import schema from "../convex/schema.js"
import { Routes } from "../src/Main.js"
import { AppConfig } from "../src/services/AppConfig.js"
import { AppStore } from "../src/services/AppStore.js"
import { Auth } from "../src/services/Auth.js"
import { Background } from "../src/services/Background.js"
import { Blob } from "../src/services/Blob.js"
import { Gateway, type GatewayService } from "../src/services/Gateway.js"
import { Store } from "../src/services/Store.js"
import { Stripe } from "../src/services/Stripe.js"
import { Transcriber } from "../src/services/Transcriber.js"
import { UpstreamUnavailable } from "../src/Errors.js"

export const SECRET = "test-secret"

const modules = import.meta.glob("../convex/**/*.ts")

export const makeConvex = async () => {
  const t = convexTest(schema, modules)
  rateLimiter.register(t)
  await t.mutation(internal.admin.seed, {})
  return t
}
export type TestConvex = Awaited<ReturnType<typeof makeConvex>>

const encoder = new TextEncoder()

/** One SSE event, as the Gateway frames it. */
export const sse = (event: Record<string, unknown>) =>
  encoder.encode(`event: ${String(event.type)}\ndata: ${JSON.stringify(event)}\n\n`)

/** A complete text answer with the given usage in its final message_delta. */
export const answer = (
  text: string,
  usage: { input: number; output: number; cacheRead?: number; cacheWrite?: number }
): Array<Uint8Array> => [
  sse({ type: "message_start", message: { id: "msg_1", type: "message", role: "assistant", content: [], usage: { input_tokens: 0, output_tokens: 0 } } }),
  sse({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }),
  sse({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text } }),
  sse({ type: "content_block_stop", index: 0 }),
  sse({
    type: "message_delta",
    delta: { stop_reason: "end_turn" },
    usage: { input_tokens: usage.input, output_tokens: usage.output },
    provider_metadata: {
      anthropic: {
        usage: {
          input_tokens: usage.input,
          output_tokens: usage.output,
          cache_read_input_tokens: usage.cacheRead ?? 0,
          cache_creation_input_tokens: usage.cacheWrite ?? 0
        }
      }
    }
  }),
  sse({ type: "message_stop" })
]

export interface FakeGateway {
  readonly requests: Array<any>
  /** What the next requests stream back; defaults to a short answer. */
  respond: (body: any) => ReturnType<GatewayService["stream"]>
}

export const fakeGateway = (): FakeGateway & { layer: Layer.Layer<Gateway> } => {
  const fake: FakeGateway = {
    requests: [],
    respond: () => Effect.succeed({ body: Stream.fromIterable(answer("Hello.", { input: 10, output: 5 })) })
  }
  const layer = Layer.succeed(
    Gateway,
    Gateway.of({
      stream: (body) => {
        fake.requests.push(body)
        return fake.respond(body)
      }
    })
  )
  return Object.assign(fake, { layer })
}

const unavailable = Effect.fail(new UpstreamUnavailable({ message: "not in tests" }))

export const vendorFakes = Layer.mergeAll(
  Layer.succeed(Blob, Blob.of({ uploadTarget: () => unavailable, size: () => unavailable, transcriberSource: () => unavailable, delete: () => Effect.void })),
  Layer.succeed(Transcriber, Transcriber.of({ start: () => unavailable, get: () => unavailable, delete: () => Effect.void })),
  Layer.succeed(AppStore, AppStore.of({ verifyTransaction: () => unavailable, verifyNotification: () => unavailable, history: () => unavailable })),
  Layer.succeed(
    Stripe,
    Stripe.of({
      ensureCustomer: () => unavailable,
      checkout: () => unavailable,
      portal: () => unavailable,
      parseEvent: () => unavailable,
      subscription: () => unavailable,
      deleteCustomer: () => Effect.void
    })
  )
)

/** Store over a convex-test instance: the proxy's calls run the real Convex functions. */
export const storeFor = (t: TestConvex) =>
  Store.fromCaller(
    {
      query: (ref, args) => t.query(ref, args as never),
      mutation: (ref, args) => t.mutation(ref, args as never),
      action: (ref, args) => t.action(ref, args as never)
    },
    SECRET
  )

/** A web handler for the whole app over the given layers. */
export const makeApp = (layers: {
  readonly t: TestConvex
  readonly gateway?: Layer.Layer<Gateway>
  readonly vendors?: Layer.Layer<Blob | Transcriber | AppStore | Stripe>
  readonly config?: Parameters<typeof AppConfig.layerTest>[0]
}) => {
  const services = Layer.mergeAll(
    Auth.layerTest,
    storeFor(layers.t),
    layers.gateway ?? fakeGateway().layer,
    Background.layerTest,
    layers.vendors ?? vendorFakes
  ).pipe(Layer.provideMerge(AppConfig.layerTest(layers.config)))
  return HttpRouter.toWebHandler(Routes.pipe(Layer.provide(services), Layer.provide(HttpServer.layerServices)), {
    disableLogger: true
  })
}

export const question = (overrides: Record<string, unknown> = {}) => ({
  contract: "ask/2",
  questionId: crypto.randomUUID(),
  round: 0,
  mode: "standard",
  messages: [{ role: "user", content: [{ type: "text", text: "What is grace?" }] }],
  ...overrides
})

export const post = (path: string, body: unknown, user = "user_test", init: RequestInit = {}) =>
  new Request(`http://proxy.test${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer test:${user}`, ...(init.headers ?? {}) },
    body: typeof body === "string" ? body : JSON.stringify(body),
    ...init
  })

export const get = (path: string, user = "user_test") =>
  new Request(`http://proxy.test${path}`, { headers: { authorization: `Bearer test:${user}` } })

/** The ledger and this month's usage for a user, straight from the test database. */
export const accountOf = (t: TestConvex, clerkId: string) =>
  t.run(async (ctx) => {
    const user = await ctx.db.query("users").withIndex("by_clerk", (q) => q.eq("clerkId", clerkId)).unique()
    if (!user) return { user: null, ledger: [], usage: [], reservations: [] }
    const ledger = await ctx.db.query("ledger").withIndex("by_user_at", (q) => q.eq("userId", user._id)).collect()
    const usage = await ctx.db.query("usage").withIndex("by_user_period", (q) => q.eq("userId", user._id)).collect()
    const reservations = await ctx.db.query("reservations").withIndex("by_user", (q) => q.eq("userId", user._id)).collect()
    return { user, ledger, usage, reservations }
  })

export { api, internal }
