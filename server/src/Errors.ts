/**
 * The error contract. Every JSON error is
 * `{ "error": { "type", "message", "upgrade"?, "retryAfter"? } }`; internally each kind is a
 * tagged error, and `wire` gives HttpApi a schema that encodes it in that shape.
 */
import { Schema, SchemaTransformation } from "effect"
import { HttpServerResponse } from "effect/http"
import { HttpApiSchema } from "effect/http-api"

const fields = {
  message: Schema.String,
  upgrade: Schema.optionalKey(Schema.Boolean),
  retryAfter: Schema.optionalKey(Schema.Number)
}

/** 400: a bug in the client. */
export class InvalidRequest extends Schema.TaggedError<InvalidRequest>()("InvalidRequest", fields) {}
/** 401: refresh the token once, then sign in. */
export class Unauthenticated extends Schema.TaggedError<Unauthenticated>()("Unauthenticated", fields) {}
/** 402: the month's allowance is spent. */
export class AllowanceExhausted extends Schema.TaggedError<AllowanceExhausted>()("AllowanceExhausted", fields) {}
/** 403: Deep study or cloud transcripts on Free. */
export class PlanRequired extends Schema.TaggedError<PlanRequired>()("PlanRequired", fields) {}
/** 404: no such sermon (or not this user's). */
export class NotFound extends Schema.TaggedError<NotFound>()("NotFound", fields) {}
/** 409: an Ask contract this proxy no longer serves. */
export class ContractUnsupported extends Schema.TaggedError<ContractUnsupported>()("ContractUnsupported", fields) {}
/** 413: the conversation is too long. */
export class TooLarge extends Schema.TaggedError<TooLarge>()("TooLarge", fields) {}
/** 429: wait `retryAfter` seconds. */
export class RateLimited extends Schema.TaggedError<RateLimited>()("RateLimited", fields) {}
/** 502: a vendor answered badly. */
export class UpstreamFailed extends Schema.TaggedError<UpstreamFailed>()("UpstreamFailed", fields) {}
/** 503: a vendor (or Convex) is down or not configured. */
export class UpstreamUnavailable extends Schema.TaggedError<UpstreamUnavailable>()("UpstreamUnavailable", fields) {}

export type ApiError =
  | InvalidRequest
  | Unauthenticated
  | AllowanceExhausted
  | PlanRequired
  | NotFound
  | ContractUnsupported
  | TooLarge
  | RateLimited
  | UpstreamFailed
  | UpstreamUnavailable

const SPEC: Record<ApiError["_tag"], readonly [status: number, type: string]> = {
  InvalidRequest: [400, "invalid_request"],
  Unauthenticated: [401, "unauthenticated"],
  AllowanceExhausted: [402, "allowance_exhausted"],
  PlanRequired: [403, "plan_required"],
  NotFound: [404, "not_found"],
  ContractUnsupported: [409, "contract_unsupported"],
  TooLarge: [413, "too_large"],
  RateLimited: [429, "rate_limited"],
  UpstreamFailed: [502, "upstream_unavailable"],
  UpstreamUnavailable: [503, "upstream_unavailable"]
}

export const statusOf = (e: ApiError): number => SPEC[e._tag][0]

export interface ErrorBody {
  readonly error: {
    readonly type: string
    readonly message: string
    readonly upgrade?: boolean
    readonly retryAfter?: number
  }
}

export const bodyOf = (e: ApiError): ErrorBody => ({
  error: {
    type: SPEC[e._tag][1],
    message: e.message,
    ...(e.upgrade !== undefined ? { upgrade: e.upgrade } : {}),
    ...(e.retryAfter !== undefined ? { retryAfter: e.retryAfter } : {})
  }
})

/** The error as a JSON response, for routes outside HttpApi (Ask's SSE route and webhooks). */
export const toResponse = (e: ApiError): HttpServerResponse.HttpServerResponse =>
  HttpServerResponse.jsonUnsafe(bodyOf(e), {
    status: statusOf(e),
    headers: e.retryAfter !== undefined ? { "retry-after": String(e.retryAfter) } : undefined
  })

export type DenialError =
  | InvalidRequest
  | AllowanceExhausted
  | PlanRequired
  | RateLimited
  | NotFound
  | UpstreamUnavailable

/** The denial types Convex returns, mapped onto errors. */
export const fromDenial = (d: {
  readonly type: string
  readonly message: string
  readonly upgrade?: boolean
  readonly retryAfter?: number
}): DenialError => {
  const extra = {
    message: d.message,
    ...(d.upgrade !== undefined ? { upgrade: d.upgrade } : {}),
    ...(d.retryAfter !== undefined ? { retryAfter: d.retryAfter } : {})
  }
  switch (d.type) {
    case "allowance_exhausted":
      return new AllowanceExhausted(extra)
    case "plan_required":
      return new PlanRequired(extra)
    case "rate_limited":
      return new RateLimited(extra)
    case "not_found":
      return new NotFound(extra)
    case "upstream_unavailable":
      return new UpstreamUnavailable(extra)
    default:
      return new InvalidRequest(extra)
  }
}

const WireBody = Schema.Struct({
  error: Schema.Struct({
    type: Schema.String,
    message: Schema.String,
    upgrade: Schema.optionalKey(Schema.Boolean),
    retryAfter: Schema.optionalKey(Schema.Number)
  })
})

type ErrorClass<Tag extends ApiError["_tag"]> = Schema.Codec<
  Extract<ApiError, { _tag: Tag }>,
  { readonly _tag: Tag; readonly message: string; readonly upgrade?: boolean; readonly retryAfter?: number }
>

/**
 * An error class as HttpApi should encode it: in the contract's `{ error: {...} }` shape, with its
 * status. Decoding (for typed clients and tests) maps the shape back onto the class.
 */
export const wire = <Tag extends ApiError["_tag"]>(tag: Tag, cls: ErrorClass<Tag>) => {
  const [status, type] = SPEC[tag]
  return WireBody.pipe(
    Schema.decodeTo(
      cls,
      SchemaTransformation.transform({
        decode: (w: typeof WireBody.Type) => ({
          _tag: tag,
          message: w.error.message,
          ...(w.error.upgrade !== undefined ? { upgrade: w.error.upgrade } : {}),
          ...(w.error.retryAfter !== undefined ? { retryAfter: w.error.retryAfter } : {})
        }),
        encode: (e) => ({
          error: {
            type,
            message: e.message,
            ...(e.upgrade !== undefined ? { upgrade: e.upgrade } : {}),
            ...(e.retryAfter !== undefined ? { retryAfter: e.retryAfter } : {})
          }
        })
      })
    ),
    HttpApiSchema.status(status)
  )
}

export const Wire = {
  InvalidRequest: wire("InvalidRequest", InvalidRequest),
  Unauthenticated: wire("Unauthenticated", Unauthenticated),
  AllowanceExhausted: wire("AllowanceExhausted", AllowanceExhausted),
  PlanRequired: wire("PlanRequired", PlanRequired),
  NotFound: wire("NotFound", NotFound),
  RateLimited: wire("RateLimited", RateLimited),
  UpstreamFailed: wire("UpstreamFailed", UpstreamFailed),
  UpstreamUnavailable: wire("UpstreamUnavailable", UpstreamUnavailable)
}
