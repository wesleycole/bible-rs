import { Clock, Effect } from "effect"
import { effectInternalAction, effectInternalMutation } from "./lib/effect.js"
import { MutationCtx } from "./lib/services.js"
import { DAY } from "./lib/time.js"

/** Questions only pin a model for their tool loop; a week is far longer than any loop. */
export const pruneQuestions = effectInternalMutation({
  args: {},
  handler: Effect.fn("maintenance.pruneQuestions")(function*() {
    const ctx = yield* MutationCtx
    const cutoff = (yield* Clock.currentTimeMillis) - 7 * DAY
    const old = yield* Effect.promise(() => ctx.db.query("questions").withIndex("by_started", (q) => q.lt("startedAt", cutoff)).take(2000))
    for (const q of old) yield* Effect.promise(() => ctx.db.delete(q._id))
    return { deleted: old.length }
  })
})

/** The ledger is kept 13 months, long enough to compare a month with the same month last year. */
export const pruneLedger = effectInternalMutation({
  args: {},
  handler: Effect.fn("maintenance.pruneLedger")(function*() {
    const ctx = yield* MutationCtx
    const cutoff = (yield* Clock.currentTimeMillis) - 396 * DAY
    const old = yield* Effect.promise(() => ctx.db.query("ledger").withIndex("by_at", (q) => q.lt("at", cutoff)).take(2000))
    for (const r of old) yield* Effect.promise(() => ctx.db.delete(r._id))
    return { deleted: old.length }
  })
})

/**
 * Asks the proxy to do vendor work Convex has no credentials for. LINGER_PROXY_URL is the proxy's
 * base URL; without it (a developer's deployment with no proxy running) the call is skipped. A
 * failure is a defect: the cron's run fails and Convex logs it.
 */
const callProxy = (path: string, body: unknown) =>
  Effect.promise(async (): Promise<unknown> => {
    const base = process.env.LINGER_PROXY_URL
    const secret = process.env.LINGER_SERVER_SECRET
    if (!base || !secret) {
      console.log(`LINGER_PROXY_URL is not set; skipping ${path}`)
      return null
    }
    const res = await fetch(new URL(path, base), {
      method: "POST",
      headers: { authorization: `Bearer ${secret}`, "content-type": "application/json" },
      body: JSON.stringify(body)
    })
    if (!res.ok) throw new Error(`${path}: HTTP ${res.status} ${await res.text()}`)
    return res.json()
  })

/** Nightly: replays App Store notifications the webhook may have missed. */
export const reconcileAppStore = effectInternalAction({
  args: {},
  handler: Effect.fn("maintenance.reconcileAppStore")(function*() {
    const sinceMs = (yield* Clock.currentTimeMillis) - 2 * DAY
    return yield* callProxy("/v1/internal/apple/reconcile", { sinceMs })
  })
})
