/**
 * Who Convex thinks the caller is: the identity it verified from the Clerk token. The M1 tokens
 * spike calls it with a desktop OAuth access token and a Clerk session token for the same person
 * and checks that both resolve to one `tokenIdentifier`; it stays as a diagnostic.
 */
import { Effect, Schema } from "effect"
import { effectQuery } from "../lib/effect.js"
import { QueryCtx } from "../lib/services.js"

export interface Identity {
  readonly tokenIdentifier: string
  readonly subject: string
  readonly issuer: string
}

/** The caller's identity, or `null` before their first write created the `users` row. */
export const get = effectQuery({
  args: Schema.Struct({}),
  empty: () => Effect.succeed<Identity | null>(null),
  handler: Effect.fn("whoami.get")(function*() {
    const ctx = yield* QueryCtx
    const identity = yield* Effect.promise(() => ctx.auth.getUserIdentity())
    return identity && { tokenIdentifier: identity.tokenIdentifier, subject: identity.subject, issuer: identity.issuer }
  })
})
