/**
 * The change feed (docs/plans/library.md, "The sync engine"): every synced record with
 * `seq > after`, tombstones included, in `seq` order, a page at a time. Devices subscribe to it,
 * apply a page in one transaction and subscribe again from the page's `seq`; because every record
 * has its own `seq` and a user's writes commit in `seq` order, a page can end anywhere without
 * skipping one.
 */
import { Clock, Effect, Schema } from "effect"
import { type Changes, FeedRecord } from "../../shared/records.js"
import { effectMutation, effectQuery } from "../lib/effect.js"
import { CurrentUser, QueryCtx } from "../lib/services.js"

/** A page holds at most this many records… */
export const PAGE_RECORDS = 500
/** …and at most this many bytes of encoded records, whichever comes first. */
export const PAGE_BYTES = 2 * 1024 * 1024

export const ChangesArgs = Schema.Struct({
  /** The cursor: the `seq` of the last page applied, 0 for none. */
  after: Schema.Int
})

/** Encodes a stored document as its feed record; the schema keeps only the record's own fields, so `_id` and `userId` never leave. */
const encodeRecord = Schema.encodeUnknownEffect(FeedRecord)

const bytes = new TextEncoder()

export const changes = effectQuery({
  args: ChangesArgs,
  empty: ({ after }) => Effect.map(Clock.currentTimeMillis, (serverTime): Changes => ({ records: [], seq: after, serverTime })),
  handler: Effect.fn("sync.changes")(function*({ after }) {
    const ctx = yield* QueryCtx
    const { userId } = yield* CurrentUser
    const library = yield* Effect.promise(() => ctx.db.query("userLibrary").withIndex("by_user", (q) => q.eq("userId", userId)).unique())
    // Tombstones up to purgedThrough are gone, so a cursor behind them would miss deletions. A
    // device starting from 0 has nothing to delete and just pulls what is there.
    if (library && after > 0 && after < library.purgedThrough) return { resync: true } satisfies Changes
    const serverTime = yield* Clock.currentTimeMillis
    const since = { highlight: "highlights", bookmark: "bookmarks", item: "items", chat: "chats", preferences: "preferences" } as const
    const docs = yield* Effect.promise(() =>
      mergeBySeq(
        Object.entries(since).map(([type, table]) =>
          tagged(type, ctx.db.query(table).withIndex("by_user_seq", (q) => q.eq("userId", userId).gt("seq", after)))
        ),
        PAGE_RECORDS
      )
    )
    const records: Array<FeedRecord> = []
    let size = 0
    for (const doc of docs) {
      const record = yield* Effect.orDie(encodeRecord(doc))
      size += bytes.encode(JSON.stringify(record)).length
      if (records.length > 0 && size > PAGE_BYTES) break
      records.push(record)
    }
    return { records, seq: records.at(-1)?.seq ?? after, serverTime } satisfies Changes
  })
})

/**
 * The server's clock, for a device's clock offset. A mutation, because Convex serves a query's
 * cached result until a write invalidates it, `Clock` included: the feed's `serverTime` is the
 * time that query last ran, not now. It writes nothing.
 */
export const now = effectMutation({
  args: Schema.Struct({}),
  handler: () => Effect.map(Clock.currentTimeMillis, (serverTime) => ({ serverTime }))
})

type Tagged = { readonly type: string; readonly seq: number }

async function* tagged(type: string, docs: AsyncIterable<{ readonly seq: number }>): AsyncGenerator<Tagged> {
  for await (const doc of docs) yield { ...doc, type }
}

/** The first `limit` documents of several `seq`-ordered streams, merged in `seq` order, reading no further than needed. */
const mergeBySeq = async (streams: ReadonlyArray<AsyncIterator<Tagged>>, limit: number): Promise<Array<Tagged>> => {
  const heads = await Promise.all(streams.map((s) => s.next()))
  const out: Array<Tagged> = []
  while (out.length < limit) {
    let min = -1
    heads.forEach((h, i) => {
      if (!h.done && (min < 0 || h.value.seq < heads[min]!.value.seq)) min = i
    })
    if (min < 0) break
    out.push(heads[min]!.value)
    heads[min] = await streams[min]!.next()
  }
  await Promise.all(streams.map((s) => s.return?.()))
  return out
}
