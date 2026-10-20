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
  readonly find: Effect.Effect<(Stored<V> & { readonly id: K; readonly createdAt?: number | undefined }) | null>
  readonly insert: (seq: number, value: V, createdAt: number) => Effect.Effect<unknown>
  /**
   * Overwrites the record found: a value, or a tombstone at `deletedAt` for `null`. `createdAt`
   * is given only when a tombstone comes back to life; otherwise the record keeps its own.
   */
  readonly update: (id: K, seq: number, value: V | null, deletedAt: number, createdAt: number | undefined) => Effect.Effect<void>
}

export type SetResult<V> =
  | { readonly result: "ok"; readonly seq: number }
  | { readonly result: "conflict"; readonly seq: number; readonly value: V | null; readonly createdAt?: number }

/**
 * Sets the verse's record to `value`, or clears it for `null`, unless `baseSeq` is stale. A new
 * record (or a tombstone set again) is dated `createdAt`, the device's own creation time, or now.
 */
export const setPerVerse = Effect.fnUntraced(function*<V, K>(
  table: VerseTable<V, K>,
  value: V | null,
  baseSeq: number | undefined,
  createdAt: number | undefined
) {
  const stored = yield* table.find
  const decision = decidePerVerse(stored, value, baseSeq)
  switch (decision.kind) {
    case "conflict": {
      const when = stored?.createdAt
      return { result: "conflict", seq: decision.seq, value: decision.value, ...(when === undefined ? {} : { createdAt: when }) } as SetResult<V>
    }
    case "unchanged":
      return { result: "ok", seq: decision.seq } as SetResult<V>
    case "write": {
      const seq = yield* (yield* Sequence).next
      const now = yield* Clock.currentTimeMillis
      if (stored === null) {
        // decidePerVerse never writes a tombstone where there is no record.
        yield* table.insert(seq, decision.value!, createdAt ?? now)
      } else {
        const revived = stored.value === null && decision.value !== null
        yield* table.update(stored.id, seq, decision.value, now, revived ? createdAt ?? now : undefined)
      }
      return { result: "ok", seq } as SetResult<V>
    }
  }
})
