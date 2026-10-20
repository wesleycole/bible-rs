/**
 * The per-user sequence every library write stamps. Every mutation that writes a record reads and
 * bumps the one `userLibrary` row, so Convex serializes a user's library writes and `seq` order is
 * commit order: the change feed can page by it without skipping anything.
 */
import { Context, Effect, Layer } from "effect"
import { CurrentUser, MutationCtx } from "./services.js"

export class Sequence extends Context.Service<Sequence, {
  /** The next sequence number for this user, reserved by this mutation. */
  readonly next: Effect.Effect<number>
}>()("linger/convex/Sequence") {
  static readonly layer = Layer.effect(
    Sequence,
    Effect.gen(function*() {
      const ctx = yield* MutationCtx
      const { userId } = yield* CurrentUser
      const next = Effect.promise(async () => {
        const row = await ctx.db.query("userLibrary").withIndex("by_user", (q) => q.eq("userId", userId)).unique()
        if (row) {
          await ctx.db.patch(row._id, { seq: row.seq + 1 })
          return row.seq + 1
        }
        await ctx.db.insert("userLibrary", { userId, seq: 1, purgedThrough: 0, storageBytes: 0 })
        return 1
      })
      return Sequence.of({ next })
    })
  )
}
