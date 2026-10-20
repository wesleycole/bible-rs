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
  /** Moves the user's storage counter by `delta` bytes, as files are attached and deleted. */
  readonly addStorage: (delta: number) => Effect.Effect<void>
}>()("linger/convex/Sequence") {
  static readonly layer = Layer.effect(
    Sequence,
    Effect.gen(function*() {
      const ctx = yield* MutationCtx
      const { userId } = yield* CurrentUser
      const row = async () => {
        const found = await ctx.db.query("userLibrary").withIndex("by_user", (q) => q.eq("userId", userId)).unique()
        if (found) return found
        const id = await ctx.db.insert("userLibrary", { userId, seq: 0, purgedThrough: 0, storageBytes: 0 })
        return (await ctx.db.get(id))!
      }
      const next = Effect.promise(async () => {
        const r = await row()
        await ctx.db.patch(r._id, { seq: r.seq + 1 })
        return r.seq + 1
      })
      const addStorage = (delta: number) =>
        Effect.promise(async () => {
          const r = await row()
          await ctx.db.patch(r._id, { storageBytes: Math.max(0, r.storageBytes + delta) })
        })
      return Sequence.of({ next, addStorage })
    })
  )
}
