/**
 * What a write does when the client's copy may be stale (docs/plans/library.md, "Conflicts"). Pure:
 * no `ctx`, so the rule is tested without convex-test.
 *
 * Every upsert carries `baseSeq`, the `seq` the client's copy had when the edit began. For a record
 * kept one per verse (highlights, bookmarks), a server copy that has moved past it stands, and the
 * client is told `conflict` with what the server has, which its outbox treats as done.
 */

/** A per-verse record as stored: its `seq` and its value, or `null` for a tombstone. */
export interface Stored<V> {
  readonly seq: number
  readonly value: V | null
}

export type Decision<V> =
  /** The server's copy is newer than the client's: keep it and say so. */
  | { readonly kind: "conflict"; readonly seq: number; readonly value: V | null }
  /** Nothing to write (clearing what is already clear); `seq` is the record's, or 0 for none. */
  | { readonly kind: "unchanged"; readonly seq: number }
  /** Write `value` (or a tombstone for `null`) with the next `seq`. */
  | { readonly kind: "write"; readonly value: V | null }

/**
 * The decision for setting a per-verse record to `value` (`null` to clear it). A write without
 * `baseSeq` comes from a client with no copy at all and wins.
 */
export const decidePerVerse = <V>(stored: Stored<V> | null, value: V | null, baseSeq: number | undefined): Decision<V> => {
  if (stored && baseSeq !== undefined && stored.seq > baseSeq) {
    return { kind: "conflict", seq: stored.seq, value: stored.value }
  }
  if (value === null && (stored === null || stored.value === null)) {
    return { kind: "unchanged", seq: stored?.seq ?? 0 }
  }
  return { kind: "write", value }
}
