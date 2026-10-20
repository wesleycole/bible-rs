/**
 * The error contract, shared by the proxy and the Convex functions: every failure a handler can
 * send is one of these classes, and the apps match on its `type`. The proxy encodes them as HTTP
 * statuses (src/Errors.ts); Convex handlers fail with the same classes, which convex/lib/errors.ts
 * turns into a `ConvexError` carrying the same `type`. `cargo xtask schema` generates core's
 * `ApiError` from the same list, so a type the server can send is one the apps can match.
 */
import { Schema } from "effect"

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
/** 404: no such record (or not this user's). */
export class NotFound extends Schema.TaggedError<NotFound>()("NotFound", fields) {}
/** 409: an Ask contract this proxy no longer serves. */
export class ContractUnsupported extends Schema.TaggedError<ContractUnsupported>()("ContractUnsupported", fields) {}
/** 413: the conversation is too long, or a file is over the storage limit. */
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

export const SPEC: Record<ApiError["_tag"], readonly [status: number, type: string]> = {
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
export const typeOf = (e: ApiError): string => SPEC[e._tag][1]

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
    type: typeOf(e),
    message: e.message,
    ...(e.upgrade !== undefined ? { upgrade: e.upgrade } : {}),
    ...(e.retryAfter !== undefined ? { retryAfter: e.retryAfter } : {})
  }
})
