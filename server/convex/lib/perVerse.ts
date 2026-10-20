/**
 * The write shared by every record kept one per verse (highlights, bookmarks): look the verse up,
 * apply the conflict rule, stamp the next `seq`, and insert, update or tombstone. A handler
 * describes only its own table.
 */
import { Clock, Effect } from "effect"
import { decidePerVerse, type Stored } from "./conflict.js"
import { Sequence } from "./sequence.js"

/** How one per-verse table is read and written; `K` is its document id. */
export interface VerseTable<V, K> {
  /** The stored record for the verse, tombstone or not, if there is one. */
  readonly find: Effect.Effect<(Stored<V> & { readonly id: K }) | null>
  readonly insert: (seq: number, value: V) => Effect.Effect<unknown>
  /** Overwrites the record found: a value, or a tombstone at `deletedAt` for `null`. */
  readonly update: (id: K, seq: number, value: V | null, deletedAt: number) => Effect.Effect<void>
}

export type SetResult<V> =
  | { readonly result: "ok"; readonly seq: number }
  | { readonly result: "conflict"; readonly seq: number; readonly value: V | null }

/** Sets the verse's record to `value`, or clears it for `null`, unless `baseSeq` is stale. */
export const setPerVerse = Effect.fnUntraced(function*<V, K>(table: VerseTable<V, K>, value: V | null, baseSeq: number | undefined) {
  const stored = yield* table.find
  const decision = decidePerVerse(stored, value, baseSeq)
  switch (decision.kind) {
    case "conflict":
      return { result: "conflict", seq: decision.seq, value: decision.value } as SetResult<V>
    case "unchanged":
      return { result: "ok", seq: decision.seq } as SetResult<V>
    case "write": {
      const seq = yield* (yield* Sequence).next
      if (stored === null) {
        // decidePerVerse never writes a tombstone where there is no record.
        yield* table.insert(seq, decision.value!)
      } else {
        yield* table.update(stored.id, seq, decision.value, yield* Clock.currentTimeMillis)
      }
      return { result: "ok", seq } as SetResult<V>
    }
  }
})
