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

/** A record kept by `uid` (notes, chats) as stored: its `seq`, whether it is a tombstone, and whether it already holds what is being written. */
export interface StoredByUid {
  readonly seq: number
  readonly deleted: boolean
  readonly same: boolean
}

export type UidDecision =
  /** Insert, overwrite or tombstone, with the next `seq`. */
  | { readonly kind: "write" }
  /** Nothing to write: the server already has this (a retried write), or the removal finds nothing. */
  | { readonly kind: "unchanged"; readonly seq: number }
  /** Another device changed the record after this client's copy: the server's stands. */
  | { readonly kind: "conflict"; readonly seq: number }

/**
 * The decision for writing (or, `removing`, deleting) a record kept by `uid`. A write that the
 * server already holds is answered `ok` at the stored `seq`, so a retry whose first answer was lost
 * never conflicts with itself.
 */
export const decideByUid = (stored: StoredByUid | null, removing: boolean, baseSeq: number | undefined): UidDecision => {
  if (stored === null) return removing ? { kind: "unchanged", seq: 0 } : { kind: "write" }
  if (removing ? stored.deleted : stored.same && !stored.deleted) return { kind: "unchanged", seq: stored.seq }
  if (baseSeq !== undefined && stored.seq > baseSeq) return { kind: "conflict", seq: stored.seq }
  return { kind: "write" }
}
