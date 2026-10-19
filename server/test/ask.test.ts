import { describe, expect, it, vi } from "vitest"
import { Effect, Stream } from "effect"
import { tokenCost } from "../convex/lib/pricing.js"
import { InvalidRequest, UpstreamUnavailable } from "../src/Errors.js"
import { accountOf, answer, fakeGateway, internal, makeApp, makeConvex, post, question, sse } from "./harness.js"

const read = async (res: Response) => {
  const text = await res.text()
  return text
}

/** Waits for settlement, which runs as the response stream finishes. */
const settled = async (t: Awaited<ReturnType<typeof makeConvex>>, clerkId: string, count: number) => {
  for (let i = 0; i < 100; i++) {
    const a = await accountOf(t, clerkId)
    if (a.ledger.length >= count && a.reservations.length === 0) return a
    await new Promise((r) => setTimeout(r, 20))
  }
  return accountOf(t, clerkId)
}

describe("POST /v1/ask", () => {
  it("streams the Gateway's bytes unchanged, with the contract and pinned settings injected", async () => {
    const t = await makeConvex()
    const gateway = fakeGateway()
    const events = answer("Grace is unearned favour.", { input: 12, output: 7 })
    gateway.respond = () => Effect.succeed({ body: Stream.fromIterable(events) })
    const { handler } = makeApp({ t, gateway: gateway.layer })

    const body = question()
    const res = await handler(post("/v1/ask", body, "user_a"))
    expect(res.status).toBe(200)
    expect(res.headers.get("content-type")).toBe("text/event-stream")
    expect(res.headers.get("x-linger-questions-left")).toBe("29")
    const expected = new TextDecoder().decode(Buffer.concat(events))
    expect(await read(res)).toBe(expected)

    const sent = gateway.requests[0]
    expect(sent.model).toBe("anthropic/claude-haiku-5.5")
    expect(sent.max_tokens).toBe(4000)
    expect(sent.output_config).toEqual({ effort: "low" })
    expect(sent.stream).toBe(true)
    expect(sent.cache_control).toEqual({ type: "ephemeral" })
    expect(sent.providerOptions.gateway).toEqual({ only: ["anthropic"], zeroDataRetention: true })
    expect(sent.system).toContain("study assistant in Linger")
    expect(sent.tools.map((x: { name: string }) => x.name).sort()).toEqual(
      ["get_passage", "lexicon", "original_words", "read_sermon", "search_bible", "search_sermons", "word_occurrences"]
    )
    expect(sent.messages).toEqual(body.messages)
    expect(sent.fallbacks).toBeUndefined()
  })

  it("charges a completed round its actual usage, with input and output priced apart", async () => {
    const t = await makeConvex()
    const gateway = fakeGateway()
    // Asymmetric counts: swapping input and output prices would change the cost a lot.
    const usage = { input: 1000, output: 20, cacheRead: 3000, cacheWrite: 500 }
    gateway.respond = () => Effect.succeed({ body: Stream.fromIterable(answer("Yes.", usage)) })
    const { handler } = makeApp({ t, gateway: gateway.layer })
    await read(await handler(post("/v1/ask", question(), "user_cost")))

    const { ledger, usage: month } = await settled(t, "user_cost", 1)
    // Haiku 5.5: $0.10 in, $0.50 out, $0.01 cache read, $0.125 cache write per MTok.
    const expected = Math.ceil((1000 * 100_000 + 20 * 500_000 + 3000 * 10_000 + 500 * 125_000) / 1_000_000)
    expect(expected).toBe(203)
    expect(ledger[0]).toMatchObject({
      status: "ok",
      inputTokens: 1000,
      outputTokens: 20,
      cacheReadTokens: 3000,
      cacheWriteTokens: 500,
      costMicros: expected,
      model: "anthropic/claude-haiku-5.5",
      provider: "anthropic"
    })
    expect(month[0].costMicros).toBe(expected)
    const swapped = tokenCost(
      { inputTokens: 20, outputTokens: 1000, cacheReadTokens: 3000, cacheWriteTokens: 500 },
      { input: 100_000, output: 500_000, cacheRead: 10_000, cacheWrite: 125_000 }
    )
    expect(swapped).not.toBe(expected)
  })

  it("charges the reservation when the client goes away after message_start", async () => {
    const t = await makeConvex()
    const gateway = fakeGateway()
    gateway.respond = () =>
      Effect.succeed({
        body: Stream.make(sse({ type: "message_start", message: { usage: { input_tokens: 0, output_tokens: 0 } } })).pipe(
          Stream.concat(Stream.never)
        )
      })
    const { handler } = makeApp({ t, gateway: gateway.layer })
    const res = await handler(post("/v1/ask", question(), "user_abort"))
    const reader = res.body!.getReader()
    await reader.read()
    await reader.cancel()

    const { ledger, usage } = await settled(t, "user_abort", 1)
    expect(ledger).toHaveLength(1)
    expect(ledger[0].status).toBe("aborted")
    expect(ledger[0].outputTokens).toBeUndefined()
    // The worst case: Haiku's 4,000 output tokens plus the input estimate as cache writes.
    expect(ledger[0].costMicros).toBeGreaterThanOrEqual(Math.ceil((4000 * 500_000) / 1_000_000))
    expect(usage[0].costMicros).toBe(ledger[0].costMicros)
  })

  it("charges nothing when the Gateway fails before the first byte", async () => {
    const t = await makeConvex()
    const gateway = fakeGateway()
    gateway.respond = () => Effect.fail(new UpstreamUnavailable({ message: "Ask is unavailable right now." }))
    const { handler } = makeApp({ t, gateway: gateway.layer })
    const res = await handler(post("/v1/ask", question(), "user_down"))
    expect(res.status).toBe(503)
    expect(await res.json()).toEqual({ error: { type: "upstream_unavailable", message: "Ask is unavailable right now." } })
    const { ledger } = await settled(t, "user_down", 1)
    expect(ledger[0]).toMatchObject({ status: "upstream_error", costMicros: 0 })
  })

  it("forwards a Gateway rejection of the conversation as invalid_request", async () => {
    const t = await makeConvex()
    const gateway = fakeGateway()
    gateway.respond = () => Effect.fail(new InvalidRequest({ message: "messages: thinking block signature invalid" }))
    const { handler } = makeApp({ t, gateway: gateway.layer })
    const res = await handler(post("/v1/ask", question(), "user_bad"))
    expect(res.status).toBe(400)
    expect((await res.json()).error.type).toBe("invalid_request")
  })

  it("turns a stream that breaks midway into an Anthropic error event and charges the reservation", async () => {
    const t = await makeConvex()
    const gateway = fakeGateway()
    const { UpstreamFailed } = await import("../src/Errors.js")
    gateway.respond = () =>
      Effect.succeed({
        body: Stream.make(sse({ type: "message_start", message: { usage: {} } })).pipe(
          Stream.concat(Stream.fail(new UpstreamFailed({ message: "The answer stopped arriving." })))
        )
      })
    const { handler } = makeApp({ t, gateway: gateway.layer })
    const text = await read(await handler(post("/v1/ask", question(), "user_break")))
    expect(text).toContain('event: error\ndata: {"type":"error","error":{"type":"api_error","message":"The answer stopped arriving."}}')
    const { ledger } = await settled(t, "user_break", 1)
    expect(ledger[0].status).toBe("upstream_error")
    expect(ledger[0].costMicros).toBeGreaterThan(0)
  })

  it("401s without a token, before touching Convex", async () => {
    const t = await makeConvex()
    const { handler } = makeApp({ t })
    const res = await handler(new Request("http://proxy.test/v1/ask", { method: "POST", body: JSON.stringify(question()) }))
    expect(res.status).toBe(401)
    expect((await res.json()).error.type).toBe("unauthenticated")
  })
})

describe("allowances", () => {
  it("lets the 30th Free question through and answers the 31st with 402", async () => {
    // Ten questions a day, on three days of one month, so only the monthly allowance decides.
    vi.useFakeTimers({ toFake: ["Date"] })
    try {
      const t = await makeConvex()
      const { handler } = makeApp({ t })
      let last: Response | undefined
      for (let i = 0; i < 30; i++) {
        vi.setSystemTime(Date.UTC(2026, 9, 5 + Math.floor(i / 10), 12, i % 10))
        last = await handler(post("/v1/ask", question(), "user_free"))
        expect(last.status, `question ${i + 1}`).toBe(200)
        await last.text()
      }
      expect(last!.headers.get("x-linger-questions-left")).toBe("0")
      vi.setSystemTime(Date.UTC(2026, 9, 9, 12))
      const denied = await handler(post("/v1/ask", question(), "user_free"))
      expect(denied.status).toBe(402)
      expect(await denied.json()).toEqual({
        error: { type: "allowance_exhausted", message: "You've used this month's free questions.", upgrade: true }
      })
      // A new month starts over.
      vi.setSystemTime(Date.UTC(2026, 10, 1, 0, 1))
      const next = await handler(post("/v1/ask", question(), "user_free"))
      expect(next.status).toBe(200)
      expect(next.headers.get("x-linger-questions-left")).toBe("29")
      await next.text()
    } finally {
      vi.useRealTimers()
    }
  })

  it("caps Free at 10 questions a day with a retryAfter until UTC midnight", async () => {
    const t = await makeConvex()
    const { handler } = makeApp({ t })
    for (let i = 0; i < 10; i++) {
      const res = await handler(post("/v1/ask", question(), "user_daily"))
      expect(res.status).toBe(200)
      await res.text()
    }
    const res = await handler(post("/v1/ask", question(), "user_daily"))
    expect(res.status).toBe(429)
    const body = await res.json()
    expect(body.error.type).toBe("rate_limited")
    expect(body.error.retryAfter).toBeGreaterThan(0)
    expect(body.error.retryAfter).toBeLessThanOrEqual(86_400)
    expect(res.headers.get("retry-after")).toBe(String(body.error.retryAfter))
  })

  it("counts later rounds of a question once, and refuses rounds of a question never begun", async () => {
    const t = await makeConvex()
    const { handler } = makeApp({ t })
    const q = question()
    await (await handler(post("/v1/ask", q, "user_rounds"))).text()
    for (const round of [1, 2]) {
      const res = await handler(post("/v1/ask", { ...q, round }, "user_rounds"))
      expect(res.status).toBe(200)
      expect(res.headers.get("x-linger-questions-left")).toBe("29")
      await res.text()
    }
    const orphan = await handler(post("/v1/ask", question({ round: 1 }), "user_rounds"))
    expect(orphan.status).toBe(400)
    // Free allows 6 rounds per question.
    const over = await handler(post("/v1/ask", { ...q, round: 6 }, "user_rounds"))
    expect(over.status).toBe(400)
    // Another user can't continue this user's question.
    const stolen = await handler(post("/v1/ask", { ...q, round: 1 }, "user_other"))
    expect(stolen.status).toBe(400)
  })

  it("refuses Deep study on Free, and counts a Deep study question as 2 on Pro on Opus", async () => {
    const t = await makeConvex()
    const gateway = fakeGateway()
    const { handler } = makeApp({ t, gateway: gateway.layer })
    const free = await handler(post("/v1/ask", question({ mode: "deep" }), "user_pro"))
    expect(free.status).toBe(403)
    expect((await free.json()).error).toMatchObject({ type: "plan_required", upgrade: true })

    await t.mutation(internal.admin.grant, { clerkId: "user_pro", days: 30 })
    const deep = await handler(post("/v1/ask", question({ mode: "deep" }), "user_pro"))
    expect(deep.status).toBe(200)
    expect(deep.headers.get("x-linger-questions-left")).toBe("148")
    await deep.text()
    expect(gateway.requests.at(-1).model).toBe("anthropic/claude-opus-5.5")
    expect(gateway.requests.at(-1).max_tokens).toBe(12000)

    const standard = await handler(post("/v1/ask", question(), "user_pro"))
    expect(standard.headers.get("x-linger-questions-left")).toBe("147")
    await standard.text()
    expect(gateway.requests.at(-1).model).toBe("anthropic/claude-sonnet-5.5")
    const { usage } = await accountOf(t, "user_pro")
    expect(usage[0]).toMatchObject({ questions: 3, deepQuestions: 1 })
  })

  it("moves Pro questions to Haiku past the cost ceiling, at round 0 only", async () => {
    const t = await makeConvex()
    const gateway = fakeGateway()
    const { handler } = makeApp({ t, gateway: gateway.layer })
    await t.mutation(internal.admin.grant, { clerkId: "user_heavy", days: 30 })
    const q = question()
    await (await handler(post("/v1/ask", q, "user_heavy"))).text()
    expect(gateway.requests.at(-1).model).toBe("anthropic/claude-sonnet-5.5")
    // Spend past the $5 ceiling.
    await t.run(async (ctx) => {
      for (const u of await ctx.db.query("usage").collect()) await ctx.db.patch(u._id, { costMicros: 5_000_000 })
    })
    // The question already going keeps its model.
    const next = await handler(post("/v1/ask", { ...q, round: 1 }, "user_heavy"))
    expect(next.headers.get("x-linger-downgraded")).toBeNull()
    await next.text()
    expect(gateway.requests.at(-1).model).toBe("anthropic/claude-sonnet-5.5")
    // A new one goes to Haiku, with a notice.
    const fresh = await handler(post("/v1/ask", question(), "user_heavy"))
    expect(fresh.status).toBe(200)
    expect(fresh.headers.get("x-linger-downgraded")).toBe("1")
    await fresh.text()
    expect(gateway.requests.at(-1).model).toBe("anthropic/claude-haiku-5.5")
  })

  it("stops Free questions at the Free cost ceiling instead of downgrading", async () => {
    const t = await makeConvex()
    const { handler } = makeApp({ t })
    await (await handler(post("/v1/ask", question(), "user_spent"))).text()
    await t.run(async (ctx) => {
      for (const u of await ctx.db.query("usage").collect()) await ctx.db.patch(u._id, { costMicros: 250_000 })
    })
    const res = await handler(post("/v1/ask", question(), "user_spent"))
    expect(res.status).toBe(402)
  })
})

describe("provider pinning", () => {
  it("keeps a question on its provider after the routing override flips, and sends new ones to the override", async () => {
    const t = await makeConvex()
    const gateway = fakeGateway()
    const { handler } = makeApp({ t, gateway: gateway.layer })
    const q = question()
    await (await handler(post("/v1/ask", q, "user_pin"))).text()
    expect(gateway.requests.at(-1).providerOptions.gateway.only).toEqual(["anthropic"])

    await t.mutation(internal.admin.setRouting, { providers: ["bedrock"], note: "drill" })

    await (await handler(post("/v1/ask", { ...q, round: 1 }, "user_pin"))).text()
    expect(gateway.requests.at(-1).providerOptions.gateway.only).toEqual(["anthropic"])
    await (await handler(post("/v1/ask", question(), "user_pin"))).text()
    expect(gateway.requests.at(-1).providerOptions.gateway.only).toEqual(["bedrock"])
  })

  it("lets a model row switch take effect for new questions only", async () => {
    const t = await makeConvex()
    const gateway = fakeGateway()
    const { handler } = makeApp({ t, gateway: gateway.layer })
    const q = question()
    await (await handler(post("/v1/ask", q, "user_switch"))).text()
    await t.run(async (ctx) => {
      const row = await ctx.db.query("models").withIndex("by_key", (x) => x.eq("key", "free")).unique()
      await ctx.db.patch(row!._id, { gatewayModel: "anthropic/claude-sonnet-5.5" })
    })
    await (await handler(post("/v1/ask", { ...q, round: 1 }, "user_switch"))).text()
    expect(gateway.requests.at(-1).model).toBe("anthropic/claude-haiku-5.5")
    await (await handler(post("/v1/ask", question(), "user_switch"))).text()
    expect(gateway.requests.at(-1).model).toBe("anthropic/claude-sonnet-5.5")
  })
})

