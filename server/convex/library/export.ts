/**
 * Export (docs/plans/m2-search-and-ask.md, "Export"): the whole library as one zip a person can
 * read without Linger. One folder an item, with a Markdown page (its details, references,
 * summary, outline and text) beside its original files. Stored as an `export` file of the user's
 * and deleted after a day.
 */
import { v } from "convex/values"
import { Clock, Effect, Schema } from "effect"
import { internal } from "../_generated/api.js"
import type { Doc } from "../_generated/dataModel.js"
import type { InvalidRequest, TooLarge } from "../../shared/Errors.js"
import { Transcript } from "../../shared/records.js"
import { effectAction, effectInternalMutation, effectInternalQuery, userServices } from "../lib/effect.js"
import { Files, sha256Hex } from "../lib/files.js"
import { ActionCtx, CurrentUser, MutationCtx, QueryCtx } from "../lib/services.js"
import { DAY } from "../lib/time.js"
import { describeVerse } from "../lib/verses.js"
import { type Entry, zip } from "../lib/zip.js"
import { stamp } from "./chunk.js"

export interface Export {
  readonly url: string
  /** When the file goes, in server milliseconds. */
  readonly expiresAt: number
}

const db = <A>(f: () => Promise<A>) => Effect.promise(f)

const EXTENSIONS: Record<string, string> = {
  "audio/mp4": "m4a",
  "audio/aac": "aac",
  "audio/mpeg": "mp3",
  "image/jpeg": "jpg",
  "image/png": "png",
  "image/heic": "heic",
  "application/json": "json",
  "text/plain": "txt"
}

/** A file name from a title: plain letters, digits and dashes. */
const slug = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 60) || "untitled"

const describeRange = (from: number, to: number) => (from === to ? describeVerse(from) : `${describeVerse(from)}–${describeVerse(to)}`)

/** The Markdown page of an item; `text` is a note's own or a sermon's transcript, already laid out. */
export const markdown = (item: Doc<"items">, text: string): string => {
  const lines = [`# ${item.title || "Untitled"}`, "", `- Kind: ${item.kind}`, `- Date: ${new Date(item.details?.startedAt ?? item.capturedAt).toISOString()}`]
  if (item.details?.speaker) lines.push(`- Speaker: ${item.details.speaker}`)
  if (item.details?.from) lines.push(`- From: ${item.details.from}`)
  if (item.anchors.length > 0) {
    lines.push("- References:")
    for (const a of item.anchors) {
      const where = a.atMs !== undefined ? ` at ${stamp(a.atMs)}` : a.page !== undefined ? ` on page ${a.page}` : ""
      lines.push(`  - ${describeRange(a.from, a.to)}${where}${a.origin === "model" ? " (found by a model)" : ""}`)
    }
  }
  if (item.summary) lines.push("", "## Summary", "", item.summary)
  if (item.outline) lines.push("", "## Outline", "", item.outline)
  if (text.trim() !== "") lines.push("", item.kind === "note" ? "## Note" : "## Text", "", text)
  return lines.join("\n") + "\n"
}

/** The library as the action reads it: every live item with its files. */
export const manifest = effectInternalQuery({
  args: { userId: v.id("users") },
  handler: Effect.fn("export.manifest")(function*({ userId }) {
    const ctx = yield* QueryCtx
    const items = yield* db(() => ctx.db.query("items").withIndex("by_user_seq", (q) => q.eq("userId", userId)).collect())
    const out: Array<{ item: Doc<"items">; files: Array<Doc<"files">> }> = []
    for (const item of items) {
      if (item.deletedAt !== undefined) continue
      const files = yield* db(() => ctx.db.query("files").withIndex("by_owner", (q) => q.eq("userId", userId).eq("ownerUid", item.uid)).collect())
      out.push({ item, files })
    }
    return out
  })
})

/** Builds the zip, stores it as the user's `export` file (a day's worth), and answers its link. */
export const start = effectAction({
  args: Schema.Struct({}),
  empty: () => Effect.succeed<Export | null>(null),
  handler: Effect.fn("export.start")(function*(): Effect.fn.Return<Export | null, InvalidRequest | TooLarge, ActionCtx | CurrentUser> {
    const ctx = yield* ActionCtx
    const { userId } = yield* CurrentUser
    const library: Array<{ item: Doc<"items">; files: Array<Doc<"files">> }> = yield* db(() => ctx.runQuery(internal.library.export.manifest, { userId }))
    const now = yield* Clock.currentTimeMillis
    const encoder = new TextEncoder()
    const entries: Array<Entry> = []
    const index: Array<string> = ["# Your Linger library", "", `Exported ${new Date(now).toISOString()}.`, ""]
    for (const { item, files } of library) {
      const folder = `${new Date(item.details?.startedAt ?? item.capturedAt).toISOString().slice(0, 10)}-${slug(item.title)}-${item.uid.slice(0, 8)}`
      let text = item.text ?? ""
      for (const file of files) {
        if (file.role === "uploadPart" || file.role === "audioPart" || file.role === "thumbnail") continue
        const blob = yield* db(() => ctx.storage.get(file.storageId))
        if (!blob) continue
        if (file.role === "text") {
          const parsed = Schema.decodeUnknownOption(Transcript)(JSON.parse(yield* db(() => blob.text())))
          if (parsed._tag === "Some") text = parsed.value.segments.map((s) => `${stamp(s.startMs)} ${s.text}`).join("\n\n")
          continue
        }
        const ext = EXTENSIONS[file.contentType] ?? "bin"
        const name = `${file.role}${file.page !== undefined ? `-${file.page}` : ""}.${ext}`
        entries.push({ name: `${folder}/${name}`, bytes: new Uint8Array(yield* db(() => blob.arrayBuffer())) })
      }
      entries.push({ name: `${folder}/${slug(item.title)}.md`, bytes: encoder.encode(markdown(item, text)) })
      index.push(`- [${item.title || "Untitled"}](${folder}/${slug(item.title)}.md) (${item.kind})`)
    }
    entries.unshift({ name: "README.md", bytes: encoder.encode(index.join("\n") + "\n") })
    const bytes = zip(entries, now)
    const sha256 = yield* db(() => sha256Hex(bytes))
    const storageId = yield* db(() => ctx.storage.store(new Blob([bytes], { type: "application/zip" })))
    const file: { uid: string; url: string } = yield* db(() => ctx.runMutation(internal.library.export.attach, { userId, storageId, sha256 }))
    yield* db(() => ctx.scheduler.runAfter(DAY, internal.library.export.expire, { userId, uid: file.uid }))
    return { url: file.url, expiresAt: now + DAY }
  })
})

export const attach = effectInternalMutation({
  args: { userId: v.id("users"), storageId: v.id("_storage"), sha256: v.string() },
  handler: Effect.fn("export.attach")(function*({ userId, storageId, sha256 }) {
    const ctx = yield* MutationCtx
    const user = (yield* db(() => ctx.db.get(userId)))!
    return yield* Effect.gen(function*() {
      const file = yield* (yield* Files).attach({ uid: sha256, role: "export", storageId, sha256 })
      return { uid: file.uid, url: file.url }
    }).pipe(Effect.provide(userServices(user)))
  })
})

/** A day later, the export goes. */
export const expire = effectInternalMutation({
  args: { userId: v.id("users"), uid: v.string() },
  handler: Effect.fn("export.expire")(function*({ userId, uid }) {
    const ctx = yield* MutationCtx
    const user = yield* db(() => ctx.db.get(userId))
    if (!user) return
    yield* Effect.flatMap(Files, (files) => files.remove(uid)).pipe(Effect.provide(userServices(user)))
  })
})
