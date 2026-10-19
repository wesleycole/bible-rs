/**
 * The proxy is Convex's only client. Every public function it calls is wrapped here and requires
 * an argument equal to the deployment's LINGER_SERVER_SECRET, compared in constant time; nothing
 * else is exposed.
 */
import { customAction, customMutation, customQuery } from "convex-helpers/server/customFunctions"
import { v } from "convex/values"
import { action, mutation, query } from "../_generated/server.js"

/** Compares two strings without returning early on the first difference. */
export const constantTimeEqual = (a: string, b: string): boolean => {
  const length = Math.max(a.length, b.length)
  let diff = a.length ^ b.length
  for (let i = 0; i < length; i++) {
    diff |= (a.charCodeAt(i) | 0) ^ (b.charCodeAt(i) | 0)
  }
  return diff === 0
}

const requireSecret = (secret: string) => {
  const expected = process.env.LINGER_SERVER_SECRET
  if (!expected || !constantTimeEqual(secret, expected)) {
    throw new Error("unauthorized")
  }
}

const secretOnly = {
  args: { secret: v.string() },
  input: async (_ctx: unknown, { secret }: { secret: string }) => {
    requireSecret(secret)
    return { ctx: {}, args: {} }
  }
}

export const serverQuery = customQuery(query, secretOnly)
export const serverMutation = customMutation(mutation, secretOnly)
export const serverAction = customAction(action, secretOnly)
