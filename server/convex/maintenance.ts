import { internal } from "./_generated/api.js"
import { internalAction, internalMutation } from "./_generated/server.js"
import { DAY } from "./lib/time.js"

/** Questions only pin a model for their tool loop; a week is far longer than any loop. */
export const pruneQuestions = internalMutation({
  args: {},
  handler: async (ctx) => {
    const old = await ctx.db
      .query("questions")
      .withIndex("by_started", (q) => q.lt("startedAt", Date.now() - 7 * DAY))
      .take(2000)
    for (const q of old) await ctx.db.delete(q._id)
    return { deleted: old.length }
  }
})

/** The ledger is kept 13 months, long enough to compare a month with the same month last year. */
export const pruneLedger = internalMutation({
  args: {},
  handler: async (ctx) => {
    const old = await ctx.db
      .query("ledger")
      .withIndex("by_at", (q) => q.lt("at", Date.now() - 396 * DAY))
      .take(2000)
    for (const r of old) await ctx.db.delete(r._id)
    return { deleted: old.length }
  }
})

/**
 * Asks the proxy to do vendor work Convex has no credentials for. LINGER_PROXY_URL is the proxy's
 * base URL; without it (a developer's deployment with no proxy running) the call is skipped.
 */
const callProxy = async (path: string, body: unknown) => {
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
}

/** Hourly: fail stuck sermons, then have the proxy delete their blobs and AssemblyAI jobs. */
export const failStuckSermons = internalAction({
  args: {},
  handler: async (ctx): Promise<{ blobPaths: string[]; assemblyIds: string[] }> => {
    const cleanup: { blobPaths: string[]; assemblyIds: string[] } = await ctx.runMutation(
      internal.sermons.failStuck,
      {}
    )
    if (cleanup.blobPaths.length || cleanup.assemblyIds.length) {
      await callProxy("/v1/internal/cleanup", cleanup)
    }
    return cleanup
  }
})

/** Nightly: replays App Store notifications the webhook may have missed. */
export const reconcileAppStore = internalAction({
  args: {},
  handler: async (): Promise<unknown> => callProxy("/v1/internal/apple/reconcile", { sinceMs: Date.now() - 2 * DAY })
})
