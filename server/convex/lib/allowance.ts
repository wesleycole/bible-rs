/**
 * Allowance (docs/plans/backend.md, "Effect in Convex"): every check, count and reservation (Ask
 * rounds, sermon seconds, library storage) and the one place `plans`, `usage` and `reservations`
 * are read and written, so there is one definition of "allowed".
 */
import { Clock, Context, Effect, Layer } from "effect"
import type { Doc, Id } from "../_generated/dataModel.js"
import {
  AllowanceExhausted,
  InvalidRequest,
  PlanRequired,
  RateLimited,
  TooLarge,
  UpstreamUnavailable
} from "../../shared/Errors.js"
import { rateLimiter } from "../rateLimits.js"
import { effectivePlan, ensureUsage, findUsage, loadModel, loadPlan, outstanding, type PlanKey } from "./accounts.js"
import { type TokenUsage, tokenCost, transcriptionCost, worstCaseRound } from "./pricing.js"
import { MutationCtx } from "./services.js"
import { MINUTE, nextDayStart, nextPeriodStart } from "./time.js"

/** The Free plan's one-time cloud trial: one sermon of up to an hour. */
export const TRIAL_SECONDS = 3600

const DEFAULT_PROVIDERS = ["anthropic"]

const seconds = (ms: number) => Math.max(1, Math.ceil(ms / 1000))

export interface RoundArgs {
  readonly questionId: string
  readonly round: number
  readonly mode: "standard" | "deep"
  readonly estimatedInputTokens: number
}

export interface RoundGranted {
  readonly reservationId: Id<"reservations">
  readonly model: Doc<"questions">["pinned"]
  readonly maxRounds: number
  readonly questionsLeft: number
  readonly downgraded: boolean
}

export interface RoundSettlement {
  readonly reservationId: Id<"reservations">
  readonly usage?: TokenUsage | undefined
  readonly status: string
  readonly ttftMs?: number | undefined
  readonly durationMs: number
}

/** What GET /v1/me shows of the plan and what is left of it. */
export interface Summary {
  readonly plan: PlanKey
  readonly renewsAt?: string | undefined
  readonly source?: string | undefined
  readonly ask: { readonly questionsLeft: number; readonly resetsAt: string; readonly deepAvailable: boolean }
  readonly sermons: { readonly hoursLeft: number; readonly trialAvailable: boolean }
  readonly library: { readonly storageUsed: number; readonly storageLimit: number }
}

type RoundError = InvalidRequest | PlanRequired | AllowanceExhausted | RateLimited | UpstreamUnavailable

export class Allowance extends Context.Service<Allowance, {
  /**
   * Begins one model round of a question: checks the allowance and rate limits, counts the
   * question at round 0 and pins its model and provider, then reserves the round's worst-case cost.
   */
  readonly beginRound: (user: Doc<"users">, args: RoundArgs) => Effect.Effect<RoundGranted, RoundError>
  /** Replaces a round's reservation with what it cost; settling twice is a no-op. */
  readonly settleRound: (
    s: RoundSettlement
  ) => Effect.Effect<{ readonly settled: false } | { readonly settled: true; readonly costMicros: number }>
  /** Crash recovery: Ask reservations that never settled are charged in full after 15 minutes. */
  readonly sweep: Effect.Effect<{ readonly swept: number }>
  /** Admits a sermon of `durationSec` for cloud transcription: Pro with hours left, or the unused trial (which this uses). */
  readonly admitSermon: (
    user: Doc<"users">,
    durationSec: number
  ) => Effect.Effect<{ readonly trial: boolean }, PlanRequired | AllowanceExhausted>
  readonly reserveSermon: (sermon: Doc<"sermons">) => Effect.Effect<void>
  /** A transcript arrived: the reservation becomes usage (unless it was the trial) and a ledger row. */
  readonly settleSermon: (sermon: Doc<"sermons">, audioSeconds: number) => Effect.Effect<void>
  /** A sermon failed: its reservation goes, and a trial is given back. */
  readonly releaseSermon: (sermon: Doc<"sermons">) => Effect.Effect<void>
  /** Refuses a file that would take the user's library past their plan's storage. */
  readonly checkStorage: (userId: Id<"users">, bytes: number) => Effect.Effect<void, TooLarge>
  readonly summary: (user: Doc<"users">) => Effect.Effect<Summary>
}>()("linger/convex/Allowance") {
  static readonly layer = Layer.effect(
    Allowance,
    Effect.gen(function*() {
      const ctx = yield* MutationCtx
      const promise = <A>(f: () => Promise<A>) => Effect.promise(f)

      const planOf = Effect.fnUntraced(function*(userId: Id<"users">, now: number) {
        const current = yield* promise(() => effectivePlan(ctx, userId, now))
        const plan = yield* promise(() => loadPlan(ctx, current.plan))
        return { ...current, row: plan }
      })

      const limit = Effect.fnUntraced(function*(userId: Id<"users">, newQuestion: boolean) {
        const round = yield* promise(() => rateLimiter.limit(ctx, "askRound", { key: userId }))
        if (!round.ok) return yield* new RateLimited({ message: "Slow down a little.", retryAfter: seconds(round.retryAfter) })
        if (newQuestion) {
          const question = yield* promise(() => rateLimiter.limit(ctx, "askQuestion", { key: userId }))
          if (!question.ok) {
            return yield* new RateLimited({ message: "Slow down a little.", retryAfter: seconds(question.retryAfter) })
          }
        }
      })

      const currentProvider = promise(async () => {
        const routing = await ctx.db.query("routing").first()
        return (routing?.providers.length ? routing.providers : DEFAULT_PROVIDERS)[0]!
      })

      const beginRound = Effect.fnUntraced(function*(user: Doc<"users">, args: RoundArgs) {
        const now = yield* Clock.currentTimeMillis
        const { plan: planKey, row: plan } = yield* planOf(user._id, now)
        const usage = yield* promise(() => ensureUsage(ctx, user._id, now))
        const existing = yield* promise(() =>
          ctx.db.query("questions").withIndex("by_question", (q) => q.eq("questionId", args.questionId)).unique()
        )
        if (existing && existing.userId !== user._id) {
          return yield* new InvalidRequest({ message: "That question belongs to someone else." })
        }
        let question: Doc<"questions">
        let downgraded = false
        // A repeated round 0 is the client retrying a question that never got going: not a new one.
        if (args.round === 0 && !existing) {
          if (args.mode === "deep" && (planKey !== "pro" || !plan.deepModelKey)) {
            return yield* new PlanRequired({ message: "Deep study is part of Linger Pro.", upgrade: true })
          }
          const cost = args.mode === "deep" ? plan.deepCost : 1
          if (usage.questions + cost > plan.questionsPerMonth) {
            return yield* new AllowanceExhausted({
              message: planKey === "free" ? "You've used this month's free questions." : "You've used this month's questions.",
              upgrade: planKey === "free"
            })
          }
          if (usage.questionsToday + 1 > plan.questionsPerDay) {
            return yield* new RateLimited({
              message: "You've asked as many questions as you can today.",
              retryAfter: seconds(nextDayStart(now) - now)
            })
          }
          let modelKey = args.mode === "deep" ? plan.deepModelKey! : plan.modelKey
          const spent = usage.costMicros + (yield* promise(() => outstanding(ctx, user._id))).micros
          if (spent >= plan.costCeilingMicros) {
            if (!plan.ceilingModelKey) {
              return yield* new AllowanceExhausted({ message: "You've used this month's questions.", upgrade: planKey === "free" })
            }
            modelKey = plan.ceilingModelKey
            downgraded = true
          }
          const model = yield* promise(() => loadModel(ctx, modelKey))
          if (!model || !model.enabled) return yield* new UpstreamUnavailable({ message: "Ask is unavailable right now." })
          yield* limit(user._id, true)
          const provider = yield* currentProvider
          const id = yield* promise(() =>
            ctx.db.insert("questions", {
              questionId: args.questionId,
              userId: user._id,
              mode: args.mode,
              pinned: {
                modelKey: model.key,
                gatewayModel: model.gatewayModel,
                provider,
                thinking: model.thinking,
                effort: model.effort,
                maxTokens: model.maxTokens,
                prices: model.prices
              },
              maxRounds: plan.maxRounds,
              rounds: 1,
              startedAt: now
            })
          )
          question = (yield* promise(() => ctx.db.get(id)))!
          yield* promise(() =>
            ctx.db.patch(usage._id, {
              questions: usage.questions + cost,
              deepQuestions: usage.deepQuestions + (args.mode === "deep" ? 1 : 0),
              questionsToday: usage.questionsToday + 1
            })
          )
          usage.questions += cost
        } else {
          if (!existing) return yield* new InvalidRequest({ message: "That question hasn't been started." })
          if (args.round >= existing.maxRounds) {
            return yield* new InvalidRequest({ message: "That question has taken as many rounds as it can." })
          }
          yield* limit(user._id, false)
          question = existing
          if (args.round + 1 > existing.rounds) yield* promise(() => ctx.db.patch(existing._id, { rounds: args.round + 1 }))
        }
        const reservationId = yield* promise(() =>
          ctx.db.insert("reservations", {
            userId: user._id,
            kind: "ask",
            amountMicros: worstCaseRound(question.pinned.maxTokens, args.estimatedInputTokens, question.pinned.prices),
            questionId: args.questionId,
            createdAt: now
          })
        )
        return {
          reservationId,
          model: question.pinned,
          maxRounds: question.maxRounds,
          questionsLeft: Math.max(0, plan.questionsPerMonth - usage.questions),
          downgraded
        }
      })

      const chargeAsk = Effect.fnUntraced(function*(
        reservation: Doc<"reservations">,
        costMicros: number,
        row: Omit<Doc<"ledger">, "_id" | "_creationTime" | "userId" | "questionId" | "kind" | "costMicros" | "at">
      ) {
        const now = yield* Clock.currentTimeMillis
        yield* promise(async () => {
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
        })
      })

      const settleRound = Effect.fnUntraced(function*(s: RoundSettlement) {
        const reservation = yield* promise(() => ctx.db.get(s.reservationId))
        if (!reservation || reservation.kind !== "ask") return { settled: false as const }
        const question = reservation.questionId
          ? yield* promise(() =>
            ctx.db.query("questions").withIndex("by_question", (q) => q.eq("questionId", reservation.questionId!)).unique()
          )
          : null
        const costMicros = s.usage && question ? tokenCost(s.usage, question.pinned.prices) : (reservation.amountMicros ?? 0)
        yield* chargeAsk(reservation, costMicros, {
          ...s.usage,
          model: question?.pinned.gatewayModel,
          provider: question?.pinned.provider,
          ttftMs: s.ttftMs,
          durationMs: s.durationMs,
          status: s.status
        })
        return { settled: true as const, costMicros }
      })

      const sweep = Effect.gen(function*() {
        const now = yield* Clock.currentTimeMillis
        const stale = yield* promise(() =>
          ctx.db.query("reservations").withIndex("by_created", (q) => q.lt("createdAt", now - 15 * MINUTE)).take(500)
        )
        let swept = 0
        for (const r of stale) {
          if (r.kind !== "ask") continue
          yield* chargeAsk(r, r.amountMicros ?? 0, { durationMs: now - r.createdAt, status: "swept" })
          swept++
        }
        return { swept }
      })

      const admitSermon = Effect.fnUntraced(function*(user: Doc<"users">, durationSec: number) {
        const now = yield* Clock.currentTimeMillis
        const { plan: planKey, row: plan } = yield* planOf(user._id, now)
        if (planKey === "pro" && plan.sermonSecondsPerMonth > 0) {
          const usage = yield* promise(() => ensureUsage(ctx, user._id, now))
          const held = yield* promise(() => outstanding(ctx, user._id))
          if (usage.sermonSeconds + held.seconds + durationSec > plan.sermonSecondsPerMonth) {
            return yield* new AllowanceExhausted({ message: "This sermon is longer than the transcription time left this month." })
          }
          return { trial: false }
        }
        if (user.trialHourUsed) {
          return yield* new PlanRequired({ message: "Cloud transcripts are part of Linger Pro.", upgrade: true })
        }
        if (durationSec > TRIAL_SECONDS) {
          return yield* new PlanRequired({ message: "The free cloud trial covers one sermon of up to an hour.", upgrade: true })
        }
        yield* promise(() => ctx.db.patch(user._id, { trialHourUsed: true }))
        return { trial: true }
      })

      const reservationFor = (s: Doc<"sermons">) =>
        promise(() =>
          ctx.db
            .query("reservations")
            .withIndex("by_user", (q) => q.eq("userId", s.userId))
            .filter((q) => q.eq(q.field("sermonId"), s._id))
            .first()
        )

      const reserveSermon = (sermon: Doc<"sermons">) =>
        Effect.asVoid(promise(() =>
          ctx.db.insert("reservations", {
            userId: sermon.userId,
            kind: "sermon",
            seconds: sermon.durationSec,
            sermonId: sermon._id,
            createdAt: sermon.createdAt
          })
        ))

      const settleSermon = Effect.fnUntraced(function*(s: Doc<"sermons">, audioSeconds: number) {
        const now = yield* Clock.currentTimeMillis
        const reservation = yield* reservationFor(s)
        yield* promise(async () => {
          if (reservation) await ctx.db.delete(reservation._id)
          if (!s.trial) {
            const usage = await ensureUsage(ctx, s.userId, now)
            await ctx.db.patch(usage._id, { sermonSeconds: usage.sermonSeconds + audioSeconds })
          }
          await ctx.db.insert("ledger", {
            userId: s.userId,
            kind: "sermon",
            audioSeconds,
            costMicros: transcriptionCost(audioSeconds),
            durationMs: now - (s.transcribingAt ?? s.createdAt),
            status: s.trial ? "trial" : "ok",
            at: now
          })
        })
      })

      const releaseSermon = Effect.fnUntraced(function*(s: Doc<"sermons">) {
        const reservation = yield* reservationFor(s)
        yield* promise(async () => {
          if (reservation) await ctx.db.delete(reservation._id)
          // A trial that failed through no fault of the user's is given back.
          if (s.trial) await ctx.db.patch(s.userId, { trialHourUsed: false })
        })
      })

      const storage = Effect.fnUntraced(function*(userId: Id<"users">, now: number) {
        const { plan, row } = yield* planOf(userId, now)
        if (row.storageBytes === undefined) return yield* Effect.die(new Error(`plan ${plan} has no storageBytes; run admin:seed`))
        const library = yield* promise(() => ctx.db.query("userLibrary").withIndex("by_user", (q) => q.eq("userId", userId)).unique())
        return { storageUsed: library?.storageBytes ?? 0, storageLimit: row.storageBytes }
      })

      const checkStorage = Effect.fnUntraced(function*(userId: Id<"users">, bytes: number) {
        const { storageUsed, storageLimit } = yield* storage(userId, yield* Clock.currentTimeMillis)
        if (storageUsed + bytes > storageLimit) {
          return yield* new TooLarge({ message: "Your library is full. Free up space, or move to Linger Pro for more.", upgrade: true })
        }
      })

      const summary = Effect.fnUntraced(function*(user: Doc<"users">) {
        const now = yield* Clock.currentTimeMillis
        const { plan: planKey, entitlement, row: plan } = yield* planOf(user._id, now)
        const usage = yield* promise(() => findUsage(ctx, user._id, now))
        const held = yield* promise(() => outstanding(ctx, user._id))
        const sermonSecondsLeft = Math.max(0, plan.sermonSecondsPerMonth - (usage?.sermonSeconds ?? 0) - held.seconds)
        return {
          plan: planKey,
          renewsAt: entitlement ? new Date(entitlement.expiresAt).toISOString() : undefined,
          source: entitlement && entitlement.source !== "grant" ? entitlement.source : undefined,
          ask: {
            questionsLeft: Math.max(0, plan.questionsPerMonth - (usage?.questions ?? 0)),
            resetsAt: new Date(nextPeriodStart(now)).toISOString(),
            deepAvailable: planKey === "pro" && plan.deepModelKey !== undefined
          },
          sermons: {
            hoursLeft: Math.round((sermonSecondsLeft / 3600) * 100) / 100,
            trialAvailable: planKey === "free" && !user.trialHourUsed
          },
          library: yield* storage(user._id, now)
        }
      })

      return Allowance.of({
        beginRound,
        settleRound,
        sweep,
        admitSermon,
        reserveSermon,
        settleSermon,
        releaseSermon,
        checkStorage,
        summary
      })
    })
  )
}
