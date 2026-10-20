/**
 * The deployment's HTTP routes (`https://<deployment>.convex.site`): the transcription provider's
 * webhook. It carries the job's own secret (convex/lib/transcriber.ts), which is checked against
 * the hash the job kept; a good one wakes the sermon's workflow.
 */
import { httpRouter } from "convex/server"
import { Option, Schema } from "effect"
import { internal } from "./_generated/api.js"
import { httpAction } from "./_generated/server.js"
import { sha256Hex } from "./lib/files.js"
import { WEBHOOK_HEADER } from "./lib/transcriber.js"

const Webhook = Schema.Struct({ transcript_id: Schema.String })
const decodeWebhook = Schema.decodeUnknownOption(Webhook)

const http = httpRouter()

http.route({
  path: "/assemblyai",
  method: "POST",
  handler: httpAction(async (ctx, request) => {
    const secret = request.headers.get(WEBHOOK_HEADER)
    const body = decodeWebhook(await request.json().catch(() => null))
    if (!secret || Option.isNone(body)) return new Response(null, { status: 400 })
    const secretHash = await sha256Hex(new TextEncoder().encode(secret))
    const known = await ctx.runMutation(internal.library.sermons.webhook, { assemblyId: body.value.transcript_id, secretHash })
    return new Response(null, { status: known ? 200 : 401 })
  })
})

export default http
