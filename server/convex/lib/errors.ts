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
