import { v } from "convex/values"
import type { Doc, Id } from "./_generated/dataModel.js"
import { internalMutation, type MutationCtx } from "./_generated/server.js"
import { effectivePlan, ensureUsage, ensureUser, loadModel, loadPlan, outstanding } from "./lib/accounts.js"
import { deny, type Denial } from "./lib/errors.js"
import { tokenCost, worstCaseRound } from "./lib/pricing.js"
import { serverMutation } from "./lib/server.js"
import { MINUTE, nextDayStart } from "./lib/time.js"
import { rateLimiter } from "./rateLimits.js"

const DEFAULT_PROVIDERS = ["anthropic"]

/** The provider new questions are pinned to: the routing override's first, or Anthropic. */
const currentProvider = async (ctx: MutationCtx) => {
  const routing = await ctx.db.query("routing").first()
  return (routing?.providers.length ? routing.providers : DEFAULT_PROVIDERS)[0]
}

const seconds = (ms: number) => Math.max(1, Math.ceil(ms / 1000))

export type BeginRound =
  | {
      ok: true
      reservationId: Id<"reservations">
      model: Doc<"questions">["pinned"]
      maxRounds: number
      questionsLeft: number
      downgraded: boolean
    }
  | Denial

/**
 * Begins one model round of a question, in one transaction: checks the allowance and rate limits,
 * counts the question at round 0 and pins its model and provider, then reserves the round's
 * worst-case cost.
 */
export const beginRound = serverMutation({
  args: {
    clerkId: v.string(),
    email: v.optional(v.string()),
    questionId: v.string(),
    round: v.number(),
    mode: v.union(v.literal("standard"), v.literal("deep")),
    estimatedInputTokens: v.number()
  },
  handler: async (ctx, args): Promise<BeginRound> => {
    const now = Date.now()
    const user = await ensureUser(ctx, args.clerkId, args.email)
    const { plan: planKey } = await effectivePlan(ctx, user._id, now)
    const plan = await loadPlan(ctx, planKey)
    const usage = await ensureUsage(ctx, user._id, now)
    const existing = await ctx.db
      .query("questions")
      .withIndex("by_question", (q) => q.eq("questionId", args.questionId))
      .unique()
    if (existing && existing.userId !== user._id) {
      return deny("invalid_request", "That question belongs to someone else.")
    }

    let question: Doc<"questions">
    let downgraded = false
    // A repeated round 0 is the client retrying a question that never got going: not a new one.
    const isNew = args.round === 0 && !existing
    if (isNew) {
      if (args.mode === "deep" && (planKey !== "pro" || !plan.deepModelKey)) {
        return deny("plan_required", "Deep study is part of Linger Pro.", { upgrade: true })
      }
      const cost = args.mode === "deep" ? plan.deepCost : 1
      if (usage.questions + cost > plan.questionsPerMonth) {
        return deny(
          "allowance_exhausted",
          planKey === "free"
            ? "You've used this month's free questions."
            : "You've used this month's questions.",
          { upgrade: planKey === "free" }
        )
      }
      if (usage.questionsToday + 1 > plan.questionsPerDay) {
        return deny("rate_limited", "You've asked as many questions as you can today.", {
          retryAfter: seconds(nextDayStart(now) - now)
        })
      }
      let modelKey = args.mode === "deep" ? plan.deepModelKey! : plan.modelKey
      const spent = usage.costMicros + (await outstanding(ctx, user._id)).micros
      if (spent >= plan.costCeilingMicros) {
        if (!plan.ceilingModelKey) {
          return deny("allowance_exhausted", "You've used this month's questions.", {
            upgrade: planKey === "free"
          })
        }
        modelKey = plan.ceilingModelKey
        downgraded = true
      }
      const model = await loadModel(ctx, modelKey)
      if (!model || !model.enabled) {
        return deny("upstream_unavailable", "Ask is unavailable right now.")
      }
      const limited = await rateLimit(ctx, user._id, true)
      if (limited) return limited
      const id = await ctx.db.insert("questions", {
        questionId: args.questionId,
        userId: user._id,
        mode: args.mode,
        pinned: {
          modelKey: model.key,
          gatewayModel: model.gatewayModel,
          provider: await currentProvider(ctx),
          thinking: model.thinking,
          effort: model.effort,
          maxTokens: model.maxTokens,
          prices: model.prices
        },
        maxRounds: plan.maxRounds,
        rounds: 1,
        startedAt: now
      })
      question = (await ctx.db.get(id))!
      await ctx.db.patch(usage._id, {
        questions: usage.questions + cost,
        deepQuestions: usage.deepQuestions + (args.mode === "deep" ? 1 : 0),
        questionsToday: usage.questionsToday + 1
      })
      usage.questions += cost
    } else {
      if (!existing) {
        return deny("invalid_request", "That question hasn't been started.")
      }
      if (args.round >= existing.maxRounds) {
        return deny("invalid_request", "That question has taken as many rounds as it can.")
      }
      const limited = await rateLimit(ctx, user._id, false)
      if (limited) return limited
      question = existing
      if (args.round + 1 > existing.rounds) {
        await ctx.db.patch(existing._id, { rounds: args.round + 1 })
      }
    }

    const reservationId = await ctx.db.insert("reservations", {
      userId: user._id,
      kind: "ask",
      amountMicros: worstCaseRound(question.pinned.maxTokens, args.estimatedInputTokens, question.pinned.prices),
      questionId: args.questionId,
      createdAt: now
    })
    return {
      ok: true,
      reservationId,
      model: question.pinned,
      maxRounds: question.maxRounds,
      questionsLeft: Math.max(0, plan.questionsPerMonth - usage.questions),
      downgraded
    }
  }
})

const rateLimit = async (ctx: MutationCtx, userId: Id<"users">, newQuestion: boolean): Promise<Denial | null> => {
  const round = await rateLimiter.limit(ctx, "askRound", { key: userId })
  if (!round.ok) {
    return deny("rate_limited", "Slow down a little.", { retryAfter: seconds(round.retryAfter) })
  }
  if (newQuestion) {
    const question = await rateLimiter.limit(ctx, "askQuestion", { key: userId })
    if (!question.ok) {
      return deny("rate_limited", "Slow down a little.", { retryAfter: seconds(question.retryAfter) })
    }
  }
  return null
}

const tokens = v.object({
  inputTokens: v.number(),
  outputTokens: v.number(),
  cacheReadTokens: v.number(),
  cacheWriteTokens: v.number()
})

/**
 * Replaces a round's reservation with what it cost. Without usage (the stream ended before its
 * final usage arrived), the reservation itself is charged. Settling twice is a no-op.
 */
export const settleRound = serverMutation({
  args: {
    reservationId: v.id("reservations"),
    usage: v.optional(tokens),
    status: v.string(),
    ttftMs: v.optional(v.number()),
    durationMs: v.number()
  },
  handler: async (ctx, args) => {
    const reservation = await ctx.db.get(args.reservationId)
    if (!reservation || reservation.kind !== "ask") return { settled: false as const }
    const question = reservation.questionId
      ? await ctx.db
          .query("questions")
          .withIndex("by_question", (q) => q.eq("questionId", reservation.questionId!))
          .unique()
      : null
    const costMicros =
      args.usage && question ? tokenCost(args.usage, question.pinned.prices) : (reservation.amountMicros ?? 0)
    await chargeAsk(ctx, reservation, costMicros, {
      ...args.usage,
      model: question?.pinned.gatewayModel,
      provider: question?.pinned.provider,
      ttftMs: args.ttftMs,
      durationMs: args.durationMs,
      status: args.status
    })
    return { settled: true as const, costMicros }
  }
})

const chargeAsk = async (
  ctx: MutationCtx,
  reservation: Doc<"reservations">,
  costMicros: number,
  row: {
    model?: string
    provider?: string
    inputTokens?: number
    outputTokens?: number
    cacheReadTokens?: number
    cacheWriteTokens?: number
    ttftMs?: number
    durationMs: number
    status: string
  }
) => {
  const now = Date.now()
  const usage = await ensureUsage(ctx, reservation.userId, now)
  await ctx.db.patch(usage._id, { costMicros: usage.costMicros + costMicros })
  await ctx.db.insert("ledger", {
    userId: reservation.userId,
    questionId: reservation.questionId,
    kind: "ask",
    costMicros,
    at: now,
    ...row
  })
  await ctx.db.delete(reservation._id)
}

/** Crash recovery: Ask reservations that never settled are charged in full after 15 minutes. */
export const sweepReservations = internalMutation({
  args: {},
  handler: async (ctx) => {
    const now = Date.now()
    const stale = await ctx.db
      .query("reservations")
      .withIndex("by_created", (q) => q.lt("createdAt", now - 15 * MINUTE))
      .take(500)
    let swept = 0
    for (const r of stale) {
      if (r.kind !== "ask") continue
      await chargeAsk(ctx, r, r.amountMicros ?? 0, { durationMs: now - r.createdAt, status: "swept" })
      swept++
    }
    return { swept }
  }
})
