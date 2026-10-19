/**
 * A stand-in for the AI Gateway's /v1/messages, for load tests: each request streams a short
 * answer trickled out over STREAM_SECONDS (default 60), the way a long thinking model would, and
 * ends with the usage a real Anthropic stream reports.
 *
 *   PORT=4100 STREAM_SECONDS=60 npx tsx scripts/mock-gateway.ts
 */
import { createServer } from "node:http"

const port = Number(process.env.PORT ?? 4100)
const seconds = Number(process.env.STREAM_SECONDS ?? 60)
const tickMs = 2000

const event = (data: Record<string, unknown>) => `event: ${String(data.type)}\ndata: ${JSON.stringify(data)}\n\n`

let open = 0
let served = 0

createServer((req, res) => {
  if (req.method !== "POST" || req.url !== "/v1/messages") {
    res.writeHead(404).end()
    return
  }
  // Drain the body; the proxy sends the full request.
  req.resume()
  req.on("end", () => {
    open++
    served++
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" })
    res.write(event({ type: "message_start", message: { id: `msg_${served}`, type: "message", role: "assistant", content: [], usage: { input_tokens: 0, output_tokens: 1 } } }))
    res.write(event({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }))
    const ticks = Math.max(1, Math.round((seconds * 1000) / tickMs))
    let n = 0
    const timer = setInterval(() => {
      n++
      res.write(event({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: `word${n} ` } }))
      if (n < ticks) return
      clearInterval(timer)
      res.write(event({ type: "content_block_stop", index: 0 }))
      res.write(
        event({
          type: "message_delta",
          delta: { stop_reason: "end_turn" },
          usage: { input_tokens: 20, output_tokens: 200 },
          provider_metadata: { anthropic: { usage: { input_tokens: 20, output_tokens: 200, cache_read_input_tokens: 0, cache_creation_input_tokens: 1700 } } }
        })
      )
      res.end(event({ type: "message_stop" }))
      open--
    }, tickMs)
    res.on("close", () => {
      if (!res.writableEnded) {
        clearInterval(timer)
        open--
      }
    })
  })
}).listen(port, "127.0.0.1", () => console.log(`mock gateway on :${port}, ${seconds}s streams`))

setInterval(() => console.log(`open=${open} served=${served}`), 10_000).unref()
