import { MINUTE, RateLimiter } from "@convex-dev/rate-limiter"
import { components } from "./_generated/api.js"

/** Token buckets per user, behind the per-IP Vercel Firewall rule. */
export const rateLimiter = new RateLimiter(components.rateLimiter, {
  askRound: { kind: "token bucket", rate: 20, period: MINUTE },
  askQuestion: { kind: "token bucket", rate: 10, period: MINUTE }
})
