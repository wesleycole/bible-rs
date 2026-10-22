import { defineSchema, defineTable } from "convex/server"
import { v } from "convex/values"
import { BookmarkFields, ChatFields, HighlightFields, ItemFields, PreferencesFields } from "../shared/records.js"

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
    ceilingModelKey: v.optional(v.string()),
    /** Library storage. Optional only until admin:seed has filled it in on every deployment. */
    storageBytes: v.optional(v.number()),
    /** Whether sermon audio is kept in the library (Pro) or stays on the device that recorded it. */
    keepAudio: v.optional(v.boolean())
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
    kind: v.union(v.literal("ask"), v.literal("sermon"), v.literal("digest")),
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

  /** The transcription job of a sermon item (library/sermons.ts): what billing and the webhook see. */
  sermons: defineTable({
    userId: v.id("users"),
    itemId: v.id("items"),
    durationSec: v.number(),
    /** Whether this sermon spends the Free account's one-time trial hour. */
    trial: v.boolean(),
    /** The uid of the item's `audio` file, named by the bytes' SHA-256; set by `finish` before the bytes are joined. */
    audio: v.optional(v.string()),
    workflowId: v.optional(v.string()),
    assemblyId: v.optional(v.string()),
    /** Hash of the per-job secret AssemblyAI echoes back on its webhook. */
    webhookSecretHash: v.optional(v.string()),
    status: v.union(
      v.literal("recording"),
      v.literal("uploading"),
      v.literal("waiting"),
      v.literal("transcribing"),
      v.literal("done"),
      v.literal("error")
    ),
    audioSeconds: v.optional(v.number()),
    createdAt: v.number(),
    transcribingAt: v.optional(v.number()),
    doneAt: v.optional(v.number()),
    error: v.optional(v.string())
  })
    .index("by_user", ["userId"])
    .index("by_item", ["itemId"])
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
    .index("by_deleted", ["deletedAt"])
    .index("by_user_verse", ["userId", "verse"]),

  bookmarks: defineTable({ userId: v.id("users"), ...BookmarkFields })
    .index("by_user_seq", ["userId", "seq"])
    .index("by_deleted", ["deletedAt"])
    .index("by_user_verse", ["userId", "verse"]),

  items: defineTable({ userId: v.id("users"), ...ItemFields })
    .index("by_user_seq", ["userId", "seq"])
    .index("by_deleted", ["deletedAt"])
    .index("by_user_uid", ["userId", "uid"]),

  /** One per user: the synced colour scheme and theme. */
  preferences: defineTable({ userId: v.id("users"), ...PreferencesFields }).index("by_user_seq", ["userId", "seq"]),

  chats: defineTable({ userId: v.id("users"), ...ChatFields })
    .index("by_user_seq", ["userId", "seq"])
    .index("by_deleted", ["deletedAt"])
    .index("by_user_uid", ["userId", "uid"]),

  /** Server-only: one row per chapter an item's anchor touches, rebuilt from the item's anchors. */
  anchors: defineTable({
    userId: v.id("users"),
    itemId: v.id("items"),
    book: v.number(),
    chapter: v.number(),
    from: v.number(),
    to: v.number(),
    origin: ItemFields.anchors.element.fields.origin,
    atMs: v.optional(v.number()),
    page: v.optional(v.number())
  })
    .index("by_user_chapter", ["userId", "book", "chapter"])
    .index("by_item", ["itemId"]),

  /**
   * Server-only: the unit of search and citation, rebuilt from an item's text whenever it changes
   * (library/chunk.ts). A note is one chunk; a sermon is paragraphs of its transcript with times.
   */
  chunks: defineTable({
    userId: v.id("users"),
    itemId: v.id("items"),
    ordinal: v.number(),
    text: v.string(),
    startMs: v.optional(v.number()),
    endMs: v.optional(v.number()),
    page: v.optional(v.number()),
    /** A passage the person marked (photos, M4): ranked above the rest of its page. */
    focus: v.boolean(),
    unreviewed: v.boolean()
  })
    .searchIndex("text", { searchField: "text", filterFields: ["userId"] })
    .index("by_item", ["itemId"])
    .index("by_user", ["userId"]),

  /**
   * The outside sources Ask draws on (docs/plans/m7-outside-sources.md): shared, read-only, written
   * only by `scripts/load-sources.ts`. A work's `version` names the load its readers see; a load in
   * progress carries a new version until `sources.commit` flips it.
   */
  sourceWorks: defineTable({
    key: v.string(),
    title: v.string(),
    author: v.string(),
    year: v.number(),
    tradition: v.string(),
    kind: v.union(v.literal("commentary"), v.literal("dictionary"), v.literal("topical"), v.literal("confession")),
    licence: v.string(),
    version: v.string()
  }).index("by_key", ["key"]),

  sourceEntries: defineTable({
    work: v.string(),
    version: v.string(),
    /** The work's own label for the entry (a passage, a term, a section), stable across loads. */
    key: v.string(),
    from: v.optional(v.number()),
    to: v.optional(v.number()),
    term: v.optional(v.string()),
    heading: v.optional(v.string()),
    text: v.string(),
    /** The verses the entry cites, in order of citation. */
    cites: v.optional(v.array(v.object({ from: v.number(), to: v.number() })))
  })
    .index("by_work_version", ["work", "version"])
    .index("by_term", ["term"])
    .searchIndex("search_text", { searchField: "text", filterFields: ["work"] }),

  /** One row per chapter an entry's range (or a confession section's proof text) touches. */
  sourceAnchors: defineTable({
    work: v.string(),
    version: v.string(),
    book: v.number(),
    chapter: v.number(),
    from: v.number(),
    to: v.number(),
    entryId: v.id("sourceEntries")
  })
    .index("by_chapter", ["book", "chapter"])
    .index("by_work_version", ["work", "version"]),

  /** Server-only: every stored file, counted in `userLibrary.storageBytes`. */
  files: defineTable({
    userId: v.id("users"),
    uid: v.string(),
    /** The item or chat it belongs to; none for an export. */
    ownerUid: v.optional(v.string()),
    role: ItemFields.files.element.fields.role,
    storageId: v.id("_storage"),
    contentType: v.string(),
    bytes: v.number(),
    sha256: v.string(),
    url: v.string(),
    partIndex: v.optional(v.number()),
    page: v.optional(v.number())
  })
    .index("by_user_uid", ["userId", "uid"])
    .index("by_owner", ["userId", "ownerUid"])
    .index("by_storage", ["storageId"]),

  /** One row per user: the write sequence, the oldest `seq` still deliverable, and the storage counter. */
  userLibrary: defineTable({
    userId: v.id("users"),
    seq: v.number(),
    purgedThrough: v.number(),
    storageBytes: v.number()
  }).index("by_user", ["userId"])
})
