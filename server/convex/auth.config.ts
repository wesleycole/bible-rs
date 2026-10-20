/**
 * One provider for every Linger token: the Clerk instance's issuer and JWKS, with no
 * `applicationID`, so Convex accepts the iOS and web session tokens and the desktop's OAuth access
 * tokens alike, and one person has one `tokenIdentifier` everywhere (docs/plans/backend.md, "Convex
 * auth"). Library functions also refuse a `sub` that isn't `user_…` (lib/user.ts).
 */
import type { AuthConfig } from "convex/server"

export default {
  providers: [
    {
      type: "customJwt",
      issuer: process.env.CLERK_JWT_ISSUER!,
      // The issuer's `/.well-known/jwks.json`; a local backend for tests sets the dev token key
      // inline as a data URI (scripts/library-local.sh). Convex requires it set on every deployment.
      jwks: process.env.CLERK_JWKS!,
      algorithm: "RS256"
    }
  ]
} satisfies AuthConfig
