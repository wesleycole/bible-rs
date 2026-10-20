/**
 * The JSON API, schema first: GET /v1/me, sermons, billing and account deletion. Ask's event
 * stream and the webhooks are plain router routes (see routes/Ask.ts and routes/Webhooks.ts).
 */
import { Schema } from "effect"
import { HttpApi, HttpApiEndpoint, HttpApiGroup, HttpApiMiddleware, HttpApiSchema, HttpApiSecurity } from "effect/http-api"
import { Wire } from "./Errors.js"
import { CurrentUserRef } from "./services/Auth.js"
import { Segment } from "./services/Transcriber.js"

export class Authorization extends HttpApiMiddleware.Service<Authorization, { provides: CurrentUserRef }>()(
  "linger/Api/Authorization",
  {
    security: { bearer: HttpApiSecurity.bearer },
    error: Wire.Unauthenticated
  }
) {}

export const Me = Schema.Struct({
  user: Schema.Struct({
    id: Schema.String,
    email: Schema.optionalKey(Schema.String),
    /** The UUID StoreKit purchases must carry. */
    appAccountToken: Schema.String
  }),
  plan: Schema.Literals(["free", "pro"]),
  renewsAt: Schema.optionalKey(Schema.String),
  source: Schema.optionalKey(Schema.Literals(["app_store", "stripe"])),
  ask: Schema.Struct({ questionsLeft: Schema.Number, resetsAt: Schema.String, deepAvailable: Schema.Boolean }),
  sermons: Schema.Struct({ hoursLeft: Schema.Number, trialAvailable: Schema.Boolean }),
  /** Library storage in bytes: what the user's files take, and what their plan allows. */
  library: Schema.Struct({ storageUsed: Schema.Number, storageLimit: Schema.Number })
})
export type Me = typeof Me.Type

/** The longest sermon accepted for cloud transcription. */
export const MAX_SERMON_SECONDS = 3 * 60 * 60

export const CreateSermon = Schema.Struct({
  recordingId: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(64)),
  durationSec: Schema.Number.check(Schema.isGreaterThan(0), Schema.isLessThanOrEqualTo(MAX_SERMON_SECONDS)),
  bytes: Schema.Int.check(Schema.isGreaterThan(44)),
  contentType: Schema.Literal("audio/wav")
})

export const SermonCreated = Schema.Struct({
  sermonId: Schema.String,
  upload: Schema.Struct({
    url: Schema.String,
    method: Schema.Literal("PUT"),
    headers: Schema.Record(Schema.String, Schema.String),
    expiresAt: Schema.Number
  }),
  trial: Schema.Boolean
})

export const SermonStatus = Schema.Literals(["uploading", "transcribing", "done", "error"])

export const Sermon = Schema.Struct({
  sermonId: Schema.String,
  status: SermonStatus,
  error: Schema.optionalKey(Schema.String),
  transcript: Schema.optionalKey(
    Schema.Struct({
      language: Schema.NullOr(Schema.String),
      durationMs: Schema.Number,
      segments: Schema.Array(Segment)
    })
  )
})
export type Sermon = typeof Sermon.Type

const Url = Schema.Struct({ url: Schema.String })

export class MeGroup extends HttpApiGroup.make("me").add(
  HttpApiEndpoint.get("me", "/v1/me", { success: Me, error: Wire.UpstreamUnavailable })
).middleware(Authorization) {}

export class SermonsGroup extends HttpApiGroup.make("sermons")
  .add(
    HttpApiEndpoint.post("create", "/v1/sermons", {
      payload: CreateSermon,
      success: SermonCreated,
      error: [
        Wire.InvalidRequest,
        Wire.AllowanceExhausted,
        Wire.PlanRequired,
        Wire.RateLimited,
        Wire.NotFound,
        Wire.UpstreamFailed,
        Wire.UpstreamUnavailable
      ]
    }),
    HttpApiEndpoint.post("transcribe", "/v1/sermons/:id/transcribe", {
      params: { id: Schema.String },
      success: Schema.Struct({ sermonId: Schema.String, status: SermonStatus }),
      error: [
        Wire.InvalidRequest,
        Wire.AllowanceExhausted,
        Wire.PlanRequired,
        Wire.RateLimited,
        Wire.NotFound,
        Wire.UpstreamFailed,
        Wire.UpstreamUnavailable
      ]
    }),
    HttpApiEndpoint.get("get", "/v1/sermons/:id", {
      params: { id: Schema.String },
      success: Sermon,
      error: [Wire.NotFound, Wire.UpstreamFailed, Wire.UpstreamUnavailable]
    })
  )
  .middleware(Authorization) {}

export class BillingGroup extends HttpApiGroup.make("billing")
  .add(
    HttpApiEndpoint.post("appleTransaction", "/v1/billing/apple/transactions", {
      payload: Schema.Struct({ jws: Schema.String }),
      success: Me,
      error: [Wire.InvalidRequest, Wire.UpstreamUnavailable]
    }),
    HttpApiEndpoint.post("stripeCheckout", "/v1/billing/stripe/checkout", {
      payload: Schema.Struct({ interval: Schema.Literals(["monthly", "yearly"]) }),
      success: Url,
      error: [Wire.InvalidRequest, Wire.UpstreamFailed, Wire.UpstreamUnavailable]
    }),
    HttpApiEndpoint.post("stripePortal", "/v1/billing/stripe/portal", {
      success: Url,
      error: [Wire.InvalidRequest, Wire.UpstreamFailed, Wire.UpstreamUnavailable]
    })
  )
  .middleware(Authorization) {}

export class AccountGroup extends HttpApiGroup.make("account").add(
  HttpApiEndpoint.delete("delete", "/v1/account", {
    success: HttpApiSchema.NoContent,
    error: Wire.UpstreamUnavailable
  })
).middleware(Authorization) {}

export class Api extends HttpApi.make("linger").add(MeGroup).add(SermonsGroup).add(BillingGroup).add(AccountGroup) {}
