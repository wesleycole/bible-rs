/**
 * The `chunks` rows of an item, replaced whenever its text changes (library/chunk.ts decides what
 * they are). Deleting and inserting is simplest and the rows are few: a sermon's hour is about
 * fifty paragraphs.
 */
import { Effect } from "effect"
import type { Doc } from "../_generated/dataModel.js"
import type { Chunk } from "../library/chunk.js"
import type { MutationCtx } from "./services.js"

export const replaceChunks = (ctx: MutationCtx["Service"], item: Doc<"items">, chunks: ReadonlyArray<Chunk>) =>
  Effect.promise(async () => {
    for (const old of await ctx.db.query("chunks").withIndex("by_item", (q) => q.eq("itemId", item._id)).collect()) {
      await ctx.db.delete(old._id)
    }
    for (const chunk of chunks) await ctx.db.insert("chunks", { userId: item.userId, itemId: item._id, ...chunk })
  })
