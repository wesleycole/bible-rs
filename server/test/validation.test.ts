import { describe, expect, it } from "vitest"
import { Effect } from "effect"
import { validateAsk } from "../src/AskRequest.js"
import { CONTRACTS, contractDigest } from "../src/Contract.js"
import { makeApp, makeConvex, post, question } from "./harness.js"

const check = (body: unknown) =>
  Effect.runPromise(validateAsk(typeof body === "string" ? body : JSON.stringify(body)).pipe(Effect.flip))

describe("Ask request validation", () => {
  it("rejects a tool the contract doesn't define", async () => {
    const e = await check(
      question({
        messages: [
          { role: "user", content: "hi" },
          { role: "assistant", content: [{ type: "tool_use", id: "t1", name: "run_shell", input: {} }] },
          { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: "ok" }] }
        ]
      })
    )
    expect(e._tag).toBe("InvalidRequest")
    expect(e.message).toContain("run_shell")
  })

  it("answers an unsupported contract version with contract_unsupported", async () => {
    const e = await check(question({ contract: "ask/0" }))
    expect(e._tag).toBe("ContractUnsupported")
  })

  it("rejects a body over 1 MB as too_large", async () => {
    const big = "x".repeat(1_000_001)
    const e = await check(question({ messages: [{ role: "user", content: big }] }))
    expect(e._tag).toBe("TooLarge")
  })

  it("rejects round ≥ the contract's max_rounds", async () => {
    const max = CONTRACTS[0].max_rounds
    expect((await check(question({ round: max })))._tag).toBe("InvalidRequest")
    await expect(Effect.runPromise(validateAsk(JSON.stringify(question({ round: max - 1 }))))).resolves.toBeDefined()
  })

  it("rejects conversations that don't alternate or don't end with the user", async () => {
    const twice = await check(question({ messages: [{ role: "user", content: "a" }, { role: "user", content: "b" }] }))
    expect(twice.message).toContain("alternate")
    const ends = await check(question({ messages: [{ role: "user", content: "a" }, { role: "assistant", content: "b" }] }))
    expect(ends._tag).toBe("InvalidRequest")
  })

  it("rejects block types outside the five Ask uses, such as images", async () => {
    const e = await check(
      question({ messages: [{ role: "user", content: [{ type: "image", source: { type: "url", url: "http://x" } }] }] })
    )
    expect(e._tag).toBe("InvalidRequest")
  })

  it("rejects a question id that isn't a UUID, and more than 200 messages", async () => {
    expect((await check(question({ questionId: "abc" })))._tag).toBe("InvalidRequest")
    const messages = Array.from({ length: 201 }, (_, i) => ({ role: i % 2 ? "assistant" : "user", content: "x" }))
    expect((await check(question({ messages })))._tag).toBe("TooLarge")
  })

  it("forwards the client's messages exactly, fields Schema doesn't know included", async () => {
    const messages = [
      { role: "user", content: "q" },
      { role: "assistant", content: [{ type: "thinking", thinking: "t", signature: "sig==" }, { type: "tool_use", id: "t1", name: "get_passage", input: { reference: "John 3:16" } }] },
      { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: "For God so loved", is_error: false }] }
    ]
    const ok = await Effect.runPromise(validateAsk(JSON.stringify(question({ messages }))))
    expect(ok.messages).toEqual(messages)
  })

  it("maps validation failures onto the JSON error contract over HTTP", async () => {
    const t = await makeConvex()
    const { handler } = makeApp({ t })
    const res = await handler(post("/v1/ask", "{not json"))
    expect(res.status).toBe(400)
    expect((await res.json()).error.type).toBe("invalid_request")
    const old = await handler(post("/v1/ask", question({ contract: "ask/0" })))
    expect(old.status).toBe(409)
    expect((await old.json()).error.type).toBe("contract_unsupported")
  })
})

describe("the Ask contract", () => {
  it("parses into the expected shape and matches the digest the Rust generator wrote", () => {
    expect(CONTRACTS.map((c) => c.version)).toEqual(["ask/4", "ask/3"])
    const c = CONTRACTS[0]
    expect(c.max_rounds).toBeGreaterThan(0)
    expect(c.system.length).toBeGreaterThan(200)
    expect(c.tools.map((t) => t.name).sort()).toEqual(
      [
        "cross_references",
        "get_passage",
        "lexicon",
        "original_words",
        "people_and_places",
        "read_sermon",
        "search_bible",
        "search_sermons",
        "word_occurrences"
      ]
    )
    for (const contract of CONTRACTS) {
      for (const tool of contract.tools) expect(tool.input_schema.type).toBe("object")
      expect(contractDigest(contract)).toBe(contract.sha256)
    }
  })

  it("still serves ask/3, and tells ask/2 apps to update", async () => {
    const old = await Effect.runPromise(validateAsk(JSON.stringify(question({ contract: "ask/3" }))))
    expect(old.contract.version).toBe("ask/3")
    expect((await check(question({ contract: "ask/2" })))._tag).toBe("ContractUnsupported")
  })

  it("would notice an edit to the contract", () => {
    const c = CONTRACTS[0]
    expect(contractDigest({ ...c, system: c.system + " " })).not.toBe(c.sha256)
  })
})

describe("every Convex function", () => {
  it("is an Effect handler: only convex/lib builds functions from Convex's own constructors", async () => {
    const { readdir, readFile } = await import("node:fs/promises")
    const root = new URL("../convex/", import.meta.url)
    const files = (await readdir(root, { recursive: true }))
      .filter((f) => f.endsWith(".ts") && !f.startsWith("_generated") && !f.startsWith("lib") && f !== "crons.ts" && !f.endsWith(".config.ts"))
    expect(files).toContain("users.ts")
    const raw = /=\s*(mutation|query|action|internalMutation|internalQuery|internalAction|httpAction|serverMutation|serverQuery|serverAction|userMutation|userQuery)\(/
    const plain = []
    for (const f of files) if (raw.test(await readFile(new URL(f, root), "utf8"))) plain.push(f)
    expect(plain).toEqual([])
  })
})
