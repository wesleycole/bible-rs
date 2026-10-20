/**
 * The per-chapter anchor index (docs/plans/library.md, "The data model"): one row per chapter an
 * item's anchor touches, rebuilt whenever the item's anchors change, so "what is anchored to this
 * chapter" is one indexed read.
 */
import { Context, Effect, Layer } from "effect"
import type { Id } from "../_generated/dataModel.js"
import type { InvalidRequest } from "../../shared/Errors.js"
import type { Item } from "../../shared/records.js"
import { CurrentUser, MutationCtx } from "./services.js"
import { splitRange } from "./verses.js"

export type Anchor = Item["anchors"][number]

export class Anchors extends Context.Service<Anchors, {
  /** Checks every anchor against the grid; an impossible one is refused, never guessed. */
  readonly check: (anchors: ReadonlyArray<Anchor>) => Effect.Effect<void, InvalidRequest>
  /** Replaces the item's index rows with its anchors' (none, for a deleted item). */
  readonly rebuild: (itemId: Id<"items">, anchors: ReadonlyArray<Anchor>) => Effect.Effect<void, InvalidRequest>
}>()("linger/convex/Anchors") {
  static readonly layer = Layer.effect(
    Anchors,
    Effect.gen(function*() {
      const ctx = yield* MutationCtx
      const { userId } = yield* CurrentUser
      const check = (anchors: ReadonlyArray<Anchor>) => Effect.forEach(anchors, (a) => splitRange(a.from, a.to), { discard: true })
      const rebuild = Effect.fnUntraced(function*(itemId: Id<"items">, anchors: ReadonlyArray<Anchor>) {
        const rows = yield* Effect.forEach(anchors, (a) =>
          Effect.map(splitRange(a.from, a.to), (spans) =>
            spans.map((s) => ({ userId, itemId, ...s, origin: a.origin, atMs: a.atMs, page: a.page }))))
        yield* Effect.promise(async () => {
          for (const old of await ctx.db.query("anchors").withIndex("by_item", (q) => q.eq("itemId", itemId)).collect()) {
            await ctx.db.delete(old._id)
          }
          for (const row of rows.flat()) await ctx.db.insert("anchors", row)
        })
      })
      return Anchors.of({ check, rebuild })
    })
  )
}
