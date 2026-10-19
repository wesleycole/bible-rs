/**
 * Clerk sign-in. The proxy accepts Clerk session tokens (iOS) and OAuth JWT access tokens (the
 * desktop app's PKCE flow), both verified locally against Clerk's key: no network call per
 * request. The verified `sub` is the only user id in the system.
 */
import { createClerkClient, verifyToken } from "@clerk/backend"
import { Context, Effect, Layer, Redacted } from "effect"
import { Unauthenticated, UpstreamUnavailable } from "../Errors.js"
import { AppConfig } from "./AppConfig.js"

export interface CurrentUser {
  readonly clerkId: string
  readonly email: string | undefined
}

export class CurrentUserRef extends Context.Service<CurrentUserRef, CurrentUser>()("linger/services/CurrentUser") {}

/** RFC 9068 `typ` values: Clerk's OAuth access tokens. */
const OAUTH_TYPES = ["at+jwt", "application/at+jwt"]

/** The JWT header's `typ`, without verifying anything. */
export const tokenType = (token: string): string | undefined => {
  try {
    const header = JSON.parse(Buffer.from(token.split(".")[0], "base64url").toString("utf8")) as { typ?: unknown }
    return typeof header.typ === "string" ? header.typ : undefined
  } catch {
    return undefined
  }
}

export interface AuthService {
  readonly verify: (token: string) => Effect.Effect<CurrentUser, Unauthenticated>
  /** Deletes the Clerk user (account deletion). */
  readonly deleteUser: (clerkId: string) => Effect.Effect<void, UpstreamUnavailable>
}

const unauthenticated = (message = "Sign in to use Ask.") => new Unauthenticated({ message })

export class Auth extends Context.Service<Auth, AuthService>()("linger/services/Auth") {
  static readonly layer = Layer.effect(
    Auth,
    Effect.gen(function*() {
      const { clerk } = yield* AppConfig
      const secretKey = clerk.secretKey ? Redacted.value(clerk.secretKey) : undefined
      const backend = secretKey ? createClerkClient({ secretKey }) : undefined

      const verify = Effect.fn("Auth.verify")(function*(token: string) {
        if (!clerk.jwtKey && !secretKey) {
          return yield* unauthenticated("Sign-in isn't configured on this server.")
        }
        const oauth = OAUTH_TYPES.includes(tokenType(token) ?? "")
        // The SDK's verifyToken throws on any failure (signature, expiry, issuer, header type).
        const payload = yield* Effect.tryPromise({
          try: () =>
            verifyToken(token, {
              ...(clerk.jwtKey ? { jwtKey: clerk.jwtKey } : { secretKey }),
              ...(clerk.issuer ? { issuer: clerk.issuer } : {}),
              // `azp` is a browser origin, only on session tokens.
              ...(!oauth && clerk.authorizedParties.length > 0
                ? { authorizedParties: [...clerk.authorizedParties] }
                : {}),
              headerType: oauth ? OAUTH_TYPES : "JWT"
            }),
          catch: (cause) => cause
        }).pipe(
          Effect.tapError((cause) => Effect.logDebug("token rejected", String(cause))),
          Effect.mapError(() => unauthenticated("Your session has expired; sign in again."))
        )
        const claims = payload as { sub?: unknown; email?: unknown; sts?: unknown }
        if (typeof claims.sub !== "string" || !claims.sub.startsWith("user_")) {
          return yield* unauthenticated()
        }
        // A session still waiting on a sign-in step (MFA, a required field) isn't signed in yet.
        if (claims.sts === "pending") return yield* unauthenticated()
        return { clerkId: claims.sub, email: typeof claims.email === "string" ? claims.email : undefined }
      })

      const deleteUser = Effect.fn("Auth.deleteUser")(function*(clerkId: string) {
        if (!backend) {
          yield* Effect.logWarning("CLERK_SECRET_KEY is not set; the Clerk user was not deleted")
          return
        }
        yield* Effect.tryPromise({
          try: () => backend.users.deleteUser(clerkId),
          catch: (cause) => cause
        }).pipe(
          Effect.catch((cause: unknown) =>
            // Already gone (deleted in Clerk first, then the webhook purged us) is fine.
            (cause as { status?: number })?.status === 404
              ? Effect.void
              : Effect.logError("Clerk deleteUser failed", cause).pipe(
                Effect.andThen(Effect.fail(new UpstreamUnavailable({ message: "Couldn't delete the account; try again." })))
              )
          )
        )
      })

      return Auth.of({ verify, deleteUser })
    })
  )

  /** Bearer tokens of the form `test:<clerkId>`, for tests. */
  static readonly layerTest = Layer.succeed(
    Auth,
    Auth.of({
      verify: (token) =>
        token.startsWith("test:user_")
          ? Effect.succeed({ clerkId: token.slice(5), email: undefined })
          : Effect.fail(unauthenticated()),
      deleteUser: () => Effect.void
    })
  )
}

/** `Authorization: Bearer <token>` → the token. */
export const bearer = (header: string | undefined): string | undefined => {
  const match = header?.match(/^Bearer\s+(.+)$/i)
  return match?.[1]?.trim() || undefined
}
