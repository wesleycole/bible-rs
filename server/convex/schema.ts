import { defineSchema, defineTable } from "convex/server"
import { v } from "convex/values"
import { BookmarkFields, HighlightFields } from "../shared/records.js"

/** Token prices for one model, in micro-dollars per million tokens. */
export const prices = v.object({
  input: v.number(),
  output: v.number(),
  cacheRead: v.number(),
  cacheWrite: v.number()
})

/** The settings a question is pinned to at round 0. */
export const pinned = v.object({
  modelKey: v.string(),
  gatewayModel: v.string(),
  provider: v.string(),
  thinking: v.union(v.literal("adaptive"), v.literal("disabled")),
  effort: v.string(),
  maxTokens: v.number(),
  prices
})

export default defineSchema({
  users: defineTable({
    clerkId: v.string(),
    email: v.optional(v.string()),
    createdAt: v.number(),
    /** The UUID StoreKit purchases carry, so App Store notifications find the user. */
    appAccountToken: v.string(),
    stripeCustomerId: v.optional(v.string()),
    trialHourUsed: v.boolean()
  })
    .index("by_clerk", ["clerkId"])
    .index("by_app_account_token", ["appAccountToken"])
    .index("by_stripe_customer", ["stripeCustomerId"]),

  plans: defineTable({
    key: v.string(),
    questionsPerMonth: v.number(),
    questionsPerDay: v.number(),
    /** What one Deep study question counts as. */
    deepCost: v.number(),
    maxRounds: v.number(),
    costCeilingMicros: v.number(),
    /** Cloud transcription each month; 0 when the plan has only the one-time trial. */
    sermonSecondsPerMonth: v.number(),
    modelKey: v.string(),
    deepModelKey: v.optional(v.string()),
    /** Where questions go past the cost ceiling; without one they stop. */
    ceilingModelKey: v.optional(v.string())
  }).index("by_key", ["key"]),

  models: defineTable({
    key: v.string(),
    gatewayModel: v.string(),
    thinking: v.union(v.literal("adaptive"), v.literal("disabled")),
    effort: v.string(),
    maxTokens: v.number(),
    enabled: v.boolean(),
    prices
  }).index("by_key", ["key"]),

  /** Singleton: the providers new questions are pinned to. */
  routing: defineTable({
    providers: v.array(v.string()),
    note: v.string()
  }),

  entitlements: defineTable({
    userId: v.id("users"),
    source: v.union(v.literal("app_store"), v.literal("stripe"), v.literal("grant")),
    productId: v.string(),
    status: v.union(
      v.literal("active"),
      v.literal("grace"),
      v.literal("billing_retry"),
      v.literal("expired"),
      v.literal("revoked"),
      v.literal("canceled")
    ),
    periodStart: v.number(),
    expiresAt: v.number(),
    /** App Store original transaction id, Stripe subscription id, or a grant's own id. */
    originalId: v.string(),
    environment: v.string(),
    updatedAt: v.number()
  })
    .index("by_user", ["userId"])
    .index("by_original", ["source", "originalId"]),

  usage: defineTable({
    userId: v.id("users"),
    /** "2026-11", UTC. */
    period: v.string(),
    questions: v.number(),
    deepQuestions: v.number(),
    costMicros: v.number(),
    sermonSeconds: v.number(),
    /** "2026-11-04", UTC: the day questionsToday counts. */
    day: v.string(),
    questionsToday: v.number()
  }).index("by_user_period", ["userId", "period"]),

  questions: defineTable({
    questionId: v.string(),
    userId: v.id("users"),
    mode: v.union(v.literal("standard"), v.literal("deep")),
    pinned,
    maxRounds: v.number(),
    /** Rounds begun so far. */
    rounds: v.number(),
    startedAt: v.number()
  })
    .index("by_question", ["questionId"])
    .index("by_started", ["startedAt"]),

  reservations: defineTable({
    userId: v.id("users"),
    kind: v.union(v.literal("ask"), v.literal("sermon")),
    amountMicros: v.optional(v.number()),
    seconds: v.optional(v.number()),
    questionId: v.optional(v.string()),
    sermonId: v.optional(v.id("sermons")),
    createdAt: v.number()
  })
    .index("by_created", ["createdAt"])
    .index("by_user", ["userId"]),

  ledger: defineTable({
    userId: v.id("users"),
    questionId: v.optional(v.string()),
    kind: v.union(v.literal("ask"), v.literal("sermon")),
    model: v.optional(v.string()),
    provider: v.optional(v.string()),
    inputTokens: v.optional(v.number()),
    cacheReadTokens: v.optional(v.number()),
    cacheWriteTokens: v.optional(v.number()),
    outputTokens: v.optional(v.number()),
    audioSeconds: v.optional(v.number()),
    costMicros: v.number(),
    ttftMs: v.optional(v.number()),
    durationMs: v.number(),
    status: v.string(),
    at: v.number()
  })
    .index("by_user_at", ["userId", "at"])
    .index("by_at", ["at"]),

  sermons: defineTable({
    userId: v.id("users"),
    recordingId: v.string(),
    blobPath: v.string(),
    durationSec: v.number(),
    bytes: v.number(),
    /** Whether this sermon spends the Free account's one-time trial hour. */
    trial: v.boolean(),
    assemblyId: v.optional(v.string()),
    /** Hash of the per-job secret AssemblyAI echoes back on its webhook. */
    webhookSecretHash: v.optional(v.string()),
    status: v.union(v.literal("uploading"), v.literal("transcribing"), v.literal("done"), v.literal("error")),
    transcript: v.optional(v.id("_storage")),
    audioSeconds: v.optional(v.number()),
    createdAt: v.number(),
    transcribingAt: v.optional(v.number()),
    doneAt: v.optional(v.number()),
    error: v.optional(v.string())
  })
    .index("by_user", ["userId"])
    .index("by_assembly", ["assemblyId"])
    .index("by_status", ["status", "createdAt"]),

  webhookEvents: defineTable({
    source: v.string(),
    eventId: v.string(),
    receivedAt: v.number()
  })
    .index("by_source_event", ["source", "eventId"])
    .index("by_received", ["receivedAt"]),

  // The library (M1). Synced records are the generated shapes plus the owner; every one is
  // indexed by [userId, seq] for the change feed.
  highlights: defineTable({ userId: v.id("users"), ...HighlightFields })
    .index("by_user_seq", ["userId", "seq"])
    .index("by_user_verse", ["userId", "verse"]),

  bookmarks: defineTable({ userId: v.id("users"), ...BookmarkFields })
    .index("by_user_seq", ["userId", "seq"])
    .index("by_user_verse", ["userId", "verse"]),

  /** One row per user: the write sequence, the oldest `seq` still deliverable, and the storage counter. */
  userLibrary: defineTable({
    userId: v.id("users"),
    seq: v.number(),
    purgedThrough: v.number(),
    storageBytes: v.number()
  }).index("by_user", ["userId"])
})
