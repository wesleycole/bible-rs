/**
 * The local stand-in for Vercel Blob's signed URLs (BLOB_DRIVER=local): PUT and GET of files under
 * LOCAL_BLOB_DIR, allowed only by a URL this proxy signed, within its size limit and lifetime.
 * Not mounted when the Vercel driver is in use.
 */
import { createReadStream, createWriteStream } from "node:fs"
import { stat } from "node:fs/promises"
import { Effect, Layer, Redacted, Stream } from "effect"
import { HttpRouter, HttpServerRequest, HttpServerResponse } from "effect/http"
import { NodeStream } from "@effect/platform-node"
import { AppConfig } from "../services/AppConfig.js"
import { localFile, prepareLocalFile, verifyLocalSignature } from "../services/Blob.js"

const PREFIX = "/v1/dev/blob/"

const parse = (request: HttpServerRequest.HttpServerRequest) => {
  const url = new URL(request.url, "http://local")
  return {
    path: decodeURIComponent(url.pathname.slice(PREFIX.length)),
    expires: Number(url.searchParams.get("expires")),
    max: Number(url.searchParams.get("max")),
    sig: url.searchParams.get("sig") ?? ""
  }
}

const denied = (status: number, message: string) => HttpServerResponse.jsonUnsafe({ error: { type: "forbidden", message } }, { status })

export const DevBlobRoutes = Layer.unwrap(
  Effect.gen(function*() {
    const config = yield* AppConfig
    if (config.blob.driver !== "local") return Layer.empty
    const secret = Redacted.value(config.serverSecret)
    const root = config.blob.localDir

    return HttpRouter.use((router) =>
      Effect.gen(function*() {
        yield* router.add("PUT", `${PREFIX}*`, (request) =>
          Effect.gen(function*() {
            const { path, expires, max, sig } = parse(request)
            const file = localFile(root, path)
            if (!file || !verifyLocalSignature(secret, "PUT", path, expires, max, sig)) {
              return denied(403, "Bad or expired upload URL.")
            }
            if (Number(request.headers["content-length"] ?? 0) > max) return denied(413, "Too large.")
            yield* Effect.promise(() => prepareLocalFile(file))
            const out = createWriteStream(file)
            let written = 0
            const result = yield* request.stream.pipe(
              Stream.runForEach((chunk) =>
                Effect.callback<void, Error>((resume) => {
                  written += chunk.byteLength
                  if (written > max) return resume(Effect.fail(new Error("too large")))
                  out.write(chunk, (err) => resume(err ? Effect.fail(err) : Effect.void))
                })
              ),
              Effect.result
            )
            yield* Effect.callback<void>((resume) => {
              out.end(() => resume(Effect.void))
            })
            if (result._tag === "Failure") return denied(413, "The upload exceeded its size limit.")
            return HttpServerResponse.jsonUnsafe({ pathname: path, size: written })
          }))

        yield* router.add("GET", `${PREFIX}*`, (request) =>
          Effect.gen(function*() {
            const { path, expires, max, sig } = parse(request)
            const file = localFile(root, path)
            if (!file || !verifyLocalSignature(secret, "GET", path, expires, max, sig)) {
              return denied(403, "Bad or expired URL.")
            }
            const size = yield* Effect.promise(() => stat(file).then((s) => s.size, () => undefined))
            if (size === undefined) return denied(404, "No such blob.")
            return HttpServerResponse.stream(
              NodeStream.fromReadable({ evaluate: () => createReadStream(file), onError: (e) => e }),
              { contentType: "audio/wav", contentLength: size }
            )
          }))
      })
    )
  })
)
