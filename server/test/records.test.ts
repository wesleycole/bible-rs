/**
 * The generated pairs in shared/records.ts agree: a value the Convex validator accepts is one the
 * Effect Schema decodes (docs/plans/m1-library-foundation.md, "Tests"). Values are generated from
 * each validator's own description, so a narrowing on one side only (a literal dropped, a field
 * made required) fails here.
 *
 * Convex has no integer type, so generated numbers are integers: every number in a record is one
 * (Rust's u64/u32), and the only writer of these tables is server code that writes integers.
 */
import { v, type Validator } from "convex/values"
import { Schema } from "effect"
import { describe, expect, it } from "vitest"
import * as Records from "../shared/records.js"

type Json = { type: string; value?: unknown }

/** A small seeded generator, so a failure reproduces. */
const rng = (seed: number) => () => {
  seed = (seed * 1_103_515_245 + 12_345) % 2 ** 31
  return seed / 2 ** 31
}

const sample = (json: Json, rand: () => number): unknown => {
  const pick = <A>(xs: ReadonlyArray<A>) => xs[Math.floor(rand() * xs.length)]!
  switch (json.type) {
    case "string":
      return pick(["", "a", "John 3:16", "ünïcode ✝", "x".repeat(200)])
    case "number":
      return pick([0, 1, 7, 42_003_016, 1_800_000_000_000, Number.MAX_SAFE_INTEGER])
    case "boolean":
      return rand() < 0.5
    case "null":
      return null
    case "literal":
      return json.value
    case "array":
      return Array.from({ length: Math.floor(rand() * 3) }, () => sample(json.value as Json, rand))
    case "union":
      return sample(pick(json.value as Array<Json>), rand)
    case "object": {
      const out: Record<string, unknown> = {}
      for (const [k, f] of Object.entries(json.value as Record<string, { fieldType: Json; optional: boolean }>)) {
        if (!f.optional || rand() < 0.5) out[k] = sample(f.fieldType, rand)
      }
      return out
    }
    default:
      throw new Error(`a validator of type ${json.type} isn't generated`)
  }
}

const pairs: Array<[string, Validator<any, any, any>, Schema.Codec<any, any, never, never>]> = [
  ["Highlight", v.object(Records.HighlightFields), Records.Highlight],
  ["Bookmark", v.object(Records.BookmarkFields), Records.Bookmark],
  ["Item", v.object(Records.ItemFields), Records.Item],
  ["Chat", v.object(Records.ChatFields), Records.Chat],
  ["FeedRecord", Records.FeedRecordValidator, Records.FeedRecord],
  ["Changes", Records.ChangesValidator, Records.Changes],
  ["WriteAnswer", Records.WriteAnswerValidator, Records.WriteAnswer]
]

describe("the generated records", () => {
  it.each(pairs)("%s: what the Convex validator accepts, the Effect Schema decodes", (_, validator, schema) => {
    const rand = rng(20261020)
    const decode = Schema.decodeUnknownExit(schema)
    for (let i = 0; i < 300; i++) {
      const value = sample((validator as unknown as { json: Json }).json, rand)
      const exit = decode(value)
      expect(exit._tag, JSON.stringify(value)).toBe("Success")
    }
  })
})
