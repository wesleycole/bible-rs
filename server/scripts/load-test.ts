/**
 * Opens N concurrent /v1/ask streams against a proxy (each as its own dev user, so allowances
 * don't interfere), reads them to the end, and reports status codes, time to first byte, stream
 * completion, and the proxy's memory if its pid is given. Point the proxy at scripts/mock-gateway.ts.
 *
 *   npx tsx scripts/load-test.ts --url http://127.0.0.1:3100 --n 1000 [--pid <proxy pid>]
 */
import { readFileSync } from "node:fs"
import { parseArgs } from "node:util"
import { Agent, fetch as undiciFetch } from "undici"
import { mintToken } from "./dev-token.js"

const { values } = parseArgs({
  options: { url: { type: "string", default: "http://127.0.0.1:3100" }, n: { type: "string", default: "1000" }, pid: { type: "string" } }
})
const n = Number(values.n)
const run = Date.now().toString(36)

const rssMb = (): number | undefined => {
  if (!values.pid) return undefined
  const status = readFileSync(`/proc/${values.pid}/status`, "utf8")
  const kb = Number(status.match(/VmRSS:\s+(\d+)/)?.[1])
  return Math.round(kb / 1024)
}

// One connection per stream: the default pool would queue them.
const dispatcher = new Agent({ connections: n + 10, headersTimeout: 120_000, bodyTimeout: 120_000 })

// Signed up front, so the client's own RSA signing isn't counted as the proxy's latency.
const tokens = Array.from({ length: n }, (_, i) => mintToken(`user_load_${run}_${i}`, { minutes: 30 }))

const one = async (i: number) => {
  const started = performance.now()
  const res = await undiciFetch(`${values.url}/v1/ask`, {
    method: "POST",
    dispatcher,
    headers: { "content-type": "application/json", authorization: `Bearer ${tokens[i]}` },
    body: JSON.stringify({
      contract: "ask/4",
      questionId: crypto.randomUUID(),
      round: 0,
      mode: "standard",
      messages: [{ role: "user", content: [{ type: "text", text: "What does Romans 8:28 mean?" }] }]
    })
  })
  const ttfb = performance.now() - started
  if (res.status !== 200) return { status: res.status, ttfb, complete: false, body: (await res.text()).slice(0, 200) }
  const text = await res.text()
  return { status: 200, ttfb, complete: text.includes("event: message_stop"), body: "" }
}

const baseline = rssMb()
let peak = baseline ?? 0
const sampler = setInterval(() => {
  const now = rssMb()
  if (now !== undefined) peak = Math.max(peak, now)
}, 1000)

const t0 = Date.now()
console.log(`opening ${n} streams against ${values.url}…`)
const results = await Promise.all(Array.from({ length: n }, (_, i) => one(i).catch((e: unknown) => ({ status: 0, ttfb: 0, complete: false, body: String(e) }))))
clearInterval(sampler)

const byStatus: Record<string, number> = {}
for (const r of results) byStatus[r.status] = (byStatus[r.status] ?? 0) + 1
const ttfbs = results.filter((r) => r.status === 200).map((r) => r.ttfb).sort((a, b) => a - b)
const pct = (p: number) => Math.round(ttfbs[Math.min(ttfbs.length - 1, Math.floor((p / 100) * ttfbs.length))] ?? 0)
console.log(
  JSON.stringify(
    {
      streams: n,
      wallSeconds: Math.round((Date.now() - t0) / 1000),
      byStatus,
      complete: results.filter((r) => r.complete).length,
      ttfbMs: { p50: pct(50), p95: pct(95), p99: pct(99), max: pct(100) },
      proxyRssMb: { baseline, peak },
      sampleErrors: [...new Set(results.filter((r) => r.status !== 200).map((r) => `${r.status} ${r.body}`))].slice(0, 5),
      since: t0
    },
    null,
    2
  )
)
await dispatcher.close()
