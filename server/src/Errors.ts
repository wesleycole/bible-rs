/**
 * The error contract on the wire: the shared classes (shared/Errors.ts) as HTTP responses, and
 * the schemas HttpApi uses to encode them in the `{ error: {...} }` shape.
 */
import { Schema, SchemaTransformation } from "effect"
import { HttpServerResponse } from "effect/http"
import { HttpApiSchema } from "effect/http-api"
import {
  AllowanceExhausted,
  type ApiError,
  bodyOf,
  InvalidRequest,
  NotFound,
  PlanRequired,
  RateLimited,
  SPEC,
  statusOf,
  Unauthenticated,
  UpstreamFailed,
  UpstreamUnavailable
} from "../shared/Errors.js"

export * from "../shared/Errors.js"

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
