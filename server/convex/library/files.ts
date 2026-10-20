/**
 * Uploading a file: the client asks for an upload URL, POSTs the bytes, then attaches what it
 * uploaded under its own uid (named by the bytes' SHA-256, so a retry attaches the same file once).
 */
import { Effect, Schema } from "effect"
import { Item } from "../../shared/records.js"
import { effectMutation } from "../lib/effect.js"
import { Files } from "../lib/files.js"
import { Uid } from "./notes.js"

export const uploadUrl = effectMutation({
  args: Schema.Struct({}),
  handler: Effect.fn("files.uploadUrl")(function*() {
    return yield* (yield* Files).uploadUrl
  })
})

export const AttachArgs = Schema.Struct({
  uid: Uid,
  ownerUid: Schema.optionalKey(Uid),
  role: Item.fields.files.value.fields.role,
  storageId: Schema.String,
  sha256: Schema.String,
  partIndex: Schema.optionalKey(Schema.Int),
  page: Schema.optionalKey(Schema.Int)
})

/** Records an uploaded file; answers its permanent URL. */
export const attach = effectMutation({
  args: AttachArgs,
  handler: Effect.fn("files.attach")(function*(args) {
    const file = yield* (yield* Files).attach(args)
    return { uid: file.uid, url: file.url, bytes: file.bytes }
  })
})
