/**
 * Merging search results (docs/plans/library.md, "library.search"): reciprocal-rank fusion over
 * the ranked lists, with a boost for chunks the person marked and chunks anchored to the passage
 * asked about, then grouped by item so one sermon doesn't fill every slot. Pure: no `ctx`.
 */

/** RRF's smoothing constant: the usual 60. */
const K = 60
/** What a focus or anchored chunk's score is multiplied by. */
const BOOST = 1.5

export interface Candidate {
  readonly key: string
  readonly itemKey: string
  readonly boosted: boolean
}

/**
 * Fuses the ranked lists (each best first) into one order, best first: a candidate's score is the
 * sum over the lists of 1 / (K + its rank), times BOOST when it is boosted. Grouped by item: items
 * in the order of their best candidate, with at most `perItem` of each, until `limit` in all.
 */
export const fuse = <C extends Candidate>(lists: ReadonlyArray<ReadonlyArray<C>>, limit: number, perItem: number): Array<C> => {
  const scores = new Map<string, { candidate: C; score: number }>()
  for (const list of lists) {
    list.forEach((c, rank) => {
      const entry = scores.get(c.key) ?? { candidate: c, score: 0 }
      entry.score += 1 / (K + rank + 1)
      scores.set(c.key, entry)
    })
  }
  const ranked = [...scores.values()]
    .map((e) => ({ candidate: e.candidate, score: e.candidate.boosted ? e.score * BOOST : e.score }))
    .sort((a, b) => b.score - a.score)
  const byItem = new Map<string, Array<C>>()
  for (const { candidate } of ranked) {
    const group = byItem.get(candidate.itemKey) ?? []
    if (group.length < perItem) group.push(candidate)
    byItem.set(candidate.itemKey, group)
  }
  return [...byItem.values()].flat().slice(0, limit)
}
