/**
 * Preferences: the colour scheme and theme a person chose, shown the same on each of their devices
 * (docs/plans/ios-design.md, "Appearance"). One record per user, `uid` "preferences"; the last
 * writer wins, so a write is never a conflict.
 */
import { Clock, Effect, Schema } from "effect"
import { Preferences, type WriteAnswer } from "../../shared/records.js"
import { effectMutation } from "../lib/effect.js"
import { Sequence } from "../lib/sequence.js"
import { CurrentUser, MutationCtx } from "../lib/services.js"
import { asRecord } from "./notes.js"

export const SetArgs = Schema.Struct({
  scheme: Preferences.fields.scheme,
  theme: Preferences.fields.theme
})

/** The record's id: there is one. */
export const PREFERENCES_UID = "preferences"

export const set = effectMutation({
  args: SetArgs,
  handler: Effect.fn("preferences.set")(function*(args): Effect.fn.Return<WriteAnswer, never, MutationCtx | CurrentUser | Sequence> {
    const ctx = yield* MutationCtx
    const { userId } = yield* CurrentUser
    const stored = yield* Effect.promise(() => ctx.db.query("preferences").withIndex("by_user_seq", (q) => q.eq("userId", userId)).first())
    const seq = yield* (yield* Sequence).next
    const fields = { seq, updatedAt: yield* Clock.currentTimeMillis, scheme: args.scheme, theme: args.theme }
    const id = stored
      ? yield* Effect.promise(() => ctx.db.patch(stored._id, fields).then(() => stored._id))
      : yield* Effect.promise(() => ctx.db.insert("preferences", { userId, uid: PREFERENCES_UID, ...fields }))
    const written = (yield* Effect.promise(() => ctx.db.get(id)))!
    return { result: "ok", seq, record: yield* asRecord("preferences", written) }
  })
})
