/**
 * The JSON API, schema first: GET /v1/me, billing and account deletion. Ask's event
 * stream and the webhooks are plain router routes (see routes/Ask.ts and routes/Webhooks.ts).
 */
import { Schema } from "effect"
import { HttpApi, HttpApiEndpoint, HttpApiGroup, HttpApiMiddleware, HttpApiSchema, HttpApiSecurity } from "effect/http-api"
import { Wire } from "./Errors.js"
import { CurrentUserRef } from "./services/Auth.js"

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

const Url = Schema.Struct({ url: Schema.String })

export class MeGroup extends HttpApiGroup.make("me").add(
  HttpApiEndpoint.get("me", "/v1/me", { success: Me, error: Wire.UpstreamUnavailable })
).middleware(Authorization) {}

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

export class Api extends HttpApi.make("linger").add(MeGroup).add(BillingGroup).add(AccountGroup) {}
