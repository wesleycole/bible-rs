/**
 * Who Convex thinks the caller is: the identity it verified from the Clerk token. The M1 tokens
 * spike calls it with a desktop OAuth access token and a Clerk session token for the same person
 * and checks that both resolve to one `tokenIdentifier`; it stays as a diagnostic.
 *
 * Like every library query it runs on `effectQuery`, which answers `empty` until the caller's
 * `users` row exists, so `null` here means "no write yet", not "unauthenticated": make a write
 * first (a no-op `highlights.set` clearing an unhighlighted verse will do).
 */
import { v } from "convex/values"
import { Effect, Schema } from "effect"
import { userByClerk } from "../lib/accounts.js"
import { effectInternalQuery, effectQuery } from "../lib/effect.js"
import { QueryCtx } from "../lib/services.js"

export interface Identity {
  readonly tokenIdentifier: string
  readonly subject: string
  readonly issuer: string
}

/** The caller's identity, or `null` until their first write creates the `users` row. */
export const get = effectQuery({
  args: Schema.Struct({}),
  empty: () => Effect.succeed<Identity | null>(null),
  handler: Effect.fn("whoami.get")(function*() {
    const ctx = yield* QueryCtx
    const identity = yield* Effect.promise(() => ctx.auth.getUserIdentity())
    return identity && { tokenIdentifier: identity.tokenIdentifier, subject: identity.subject, issuer: identity.issuer }
  })
})

/** The `users` row for a verified Clerk id, for an action (which has no `db` of its own); `null` before the first write. */
export const user = effectInternalQuery({
  args: { clerkId: v.string() },
  handler: Effect.fn("whoami.user")(function*({ clerkId }) {
    const ctx = yield* QueryCtx
    return yield* Effect.promise(() => userByClerk(ctx, clerkId))
  })
})
