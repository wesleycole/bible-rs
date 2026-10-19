/**
 * The Ask contract: the system prompt, tools and round cap the Rust client is built against,
 * generated from Rust by `cargo xtask contract`. The proxy injects them, byte-identical for every
 * user, so the shared prompt cache stays warm. It serves the last two versions.
 */
import { createHash } from "node:crypto"
import { Schema } from "effect"
import askV3 from "./contract/ask-v3.json" with { type: "json" }
import askV4 from "./contract/ask-v4.json" with { type: "json" }

export const Tool = Schema.Struct({
  name: Schema.String,
  description: Schema.String,
  input_schema: Schema.Record(Schema.String, Schema.Unknown)
})

export const AskContract = Schema.Struct({
  version: Schema.String,
  max_rounds: Schema.Int,
  system: Schema.String,
  tools: Schema.Array(Tool),
  sha256: Schema.String
})
export type AskContract = typeof AskContract.Type

/** JSON with object keys sorted, as serde_json writes a `Value` (its default map is ordered). */
export const canonicalJson = (value: unknown): string =>
  JSON.stringify(value, (_key, v: unknown) =>
    v !== null && typeof v === "object" && !Array.isArray(v)
      ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
      : v
  )

/** The SHA-256 of the contract's canonical bytes, as the Rust generator computes it. */
export const contractDigest = (c: AskContract): string =>
  createHash("sha256")
    .update(canonicalJson({ max_rounds: c.max_rounds, system: c.system, tools: c.tools, version: c.version }))
    .digest("hex")

const load = (raw: unknown): AskContract => {
  const c = Schema.decodeUnknownSync(AskContract)(raw)
  if (contractDigest(c) !== c.sha256) {
    throw new Error(`Ask contract ${c.version} doesn't match its digest; rerun \`cargo xtask contract\``)
  }
  return c
}

/** Newest first; the proxy serves these and answers 409 for anything older. */
export const CONTRACTS: ReadonlyArray<AskContract> = [load(askV4), load(askV3)]

export const contractFor = (version: string): AskContract | undefined => CONTRACTS.find((c) => c.version === version)

/** Rough tokens of the stable prefix (system and tools), for reserving a round's worst case. */
export const prefixTokens = (c: AskContract): number =>
  Math.ceil((c.system.length + JSON.stringify(c.tools).length) / 3)
