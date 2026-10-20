import { ConvexError } from "convex/values"
import { type ApiError, bodyOf } from "../../shared/Errors.js"

/** A typed denial, returned (not thrown) so the proxy maps it onto its error contract. */
export type Denial = {
  readonly ok: false
  readonly error: {
    readonly type:
      | "invalid_request"
      | "allowance_exhausted"
      | "plan_required"
      | "rate_limited"
      | "upstream_unavailable"
      | "not_found"
    readonly message: string
    readonly upgrade?: boolean
    readonly retryAfter?: number
  }
}

export const deny = (
  type: Denial["error"]["type"],
  message: string,
  extra: { upgrade?: boolean; retryAfter?: number } = {}
): Denial => ({ ok: false, error: { type, message, ...extra } })

/** The error as the apps see it from a Convex call: a `ConvexError` whose data is the contract's body. */
export const toConvexError = (e: ApiError) => new ConvexError(bodyOf(e).error)
