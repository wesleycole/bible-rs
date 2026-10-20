/**
 * Records Convex's own execution time for `library/highlights.set`, so the cost of Effect inside the
 * isolate is watched (docs/plans/m1-library-foundation.md, Tests). Makes N writes against the backend
 * scripts/library-local.sh started, then reads each one's `executionTime` from the deployment's log
 * stream and prints the median and p95 (to the job summary too, in CI).
 *
 *   npx tsx scripts/time-writes.ts [--n 50]     # reads LINGER_TEST_CONVEX_URL, _TOKEN, _ENV
 */
import { spawn } from "node:child_process"
import { appendFileSync } from "node:fs"
import { createInterface } from "node:readline"
import { parseArgs } from "node:util"
import { ConvexHttpClient } from "convex/browser"
import { anyApi } from "convex/server"

const { values } = parseArgs({ options: { n: { type: "string", default: "50" } } })
const n = Number(values.n)
const need = (k: string) => process.env[k] ?? (console.error(`${k} is unset: run scripts/library-local.sh`), process.exit(1))
const [url, token, envFile] = [need("LINGER_TEST_CONVEX_URL"), need("LINGER_TEST_TOKEN"), need("LINGER_TEST_CONVEX_ENV")]

// The stream first, so every write is seen as it completes (its history is a short buffer).
const logs = spawn("npx", ["convex", "logs", "--env-file", envFile, "--success", "--jsonl"], {
  stdio: ["ignore", "pipe", "pipe"]
})
await new Promise<void>((ready) => logs.stderr.on("data", (d) => String(d).includes("Watching") && ready()))
const ms: Array<{ total: number; user: number }> = []
const seen = new Set<string>()
const done = new Promise<void>((resolve) => {
  const deadline = setTimeout(resolve, 30_000)
  createInterface({ input: logs.stdout }).on("line", (line) => {
    let e: any
    try {
      e = JSON.parse(line)
    } catch {
      return
    }
    if (e.kind !== "Completion" || e.identifier !== "library/highlights:set" || seen.has(e.requestId)) return
    seen.add(e.requestId)
    ms.push({ total: e.executionTime * 1000, user: e.userExecutionTime * 1000 })
    if (ms.length >= n) {
      clearTimeout(deadline)
      resolve()
    }
  })
})

const client = new ConvexHttpClient(url)
client.setAuth(token)
const colors = ["yellow", "green", "blue", "pink", "purple"]
for (let i = 0; i < n; i++) {
  // Psalm 119:1 onwards (176 verses), a new verse and then a recolour, so both inserts and updates are timed.
  const verse = 18119001 + (Math.floor(i / 2) % 176)
  await client.mutation(anyApi.library.highlights.set, { verse, color: colors[i % colors.length] })
}
await done
logs.kill()

const at = (v: number[], q: number) => v.sort((a, b) => a - b)[Math.min(v.length - 1, Math.floor(q * v.length))]
const fmt = (v: number) => `${v.toFixed(1)} ms`
const total = ms.map((m) => m.total)
const user = ms.map((m) => m.user)
const line = `highlights.set execution time over ${ms.length} writes: median ${fmt(at(total, 0.5))}, p95 ${fmt(at(total, 0.95))} (user code: median ${fmt(at(user, 0.5))}, p95 ${fmt(at(user, 0.95))})`
console.log(line)
if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${line}\n`)
if (ms.length < n) {
  console.error(`only ${ms.length} of ${n} writes appeared in the log stream`)
  process.exit(1)
}
