/**
 * Sermon audio storage. The client PUTs the joined WAV straight to a signed URL (it never passes
 * through a function, so the 4.5 MB body limit doesn't apply), and AssemblyAI reads it through a
 * signed GET URL. Production uses a private Vercel Blob store; local development keeps files on
 * disk behind URLs this proxy signs and serves itself (see routes/DevBlob.ts).
 */
import { BlobNotFoundError, del, head, issueSignedToken, presignUrl } from "@vercel/blob"
import { createHmac, timingSafeEqual } from "node:crypto"
import { createReadStream } from "node:fs"
import { mkdir, rm, stat } from "node:fs/promises"
import { dirname, resolve } from "node:path"
import { Context, Effect, Layer, Redacted } from "effect"
import { NodeStream } from "@effect/platform-node"
import { UpstreamFailed, UpstreamUnavailable } from "../Errors.js"
import type { AudioSource } from "./Transcriber.js"
import { AppConfig } from "./AppConfig.js"

export interface UploadTarget {
  readonly url: string
  readonly method: "PUT"
  readonly headers: Record<string, string>
  readonly expiresAt: number
}

export interface BlobService {
  readonly uploadTarget: (
    path: string,
    opts: { readonly contentType: string; readonly maxBytes: number; readonly validForMs: number }
  ) => Effect.Effect<UploadTarget, UpstreamFailed | UpstreamUnavailable>
  /** The stored object's size, or undefined when nothing was uploaded. */
  readonly size: (path: string) => Effect.Effect<number | undefined, UpstreamFailed | UpstreamUnavailable>
  /** How AssemblyAI should read the audio. */
  readonly transcriberSource: (path: string) => Effect.Effect<AudioSource, UpstreamFailed | UpstreamUnavailable>
  readonly delete: (paths: ReadonlyArray<string>) => Effect.Effect<void>
}

export class Blob extends Context.Service<Blob, BlobService>()("linger/services/Blob") {
  static readonly layer = Layer.effect(
    Blob,
    Effect.gen(function*() {
      const config = yield* AppConfig
      return config.blob.driver === "vercel" ? vercelBlob(config.blob) : localBlob(config)
    })
  )
}

const unavailable = (what: string) => (cause: unknown) =>
  Effect.logError(`Blob ${what} failed`, cause).pipe(
    Effect.andThen(Effect.fail(new UpstreamFailed({ message: "Audio storage is unavailable right now." })))
  )

const vercelBlob = (cfg: { readonly token: Redacted.Redacted<string> | undefined; readonly storeId: string | undefined }) => {
  // With a read-write token the SDK uses it; on Vercel without one it uses OIDC and the store id.
  const auth = cfg.token ? { token: Redacted.value(cfg.token) } : {}
  const signed = (path: string, operations: Array<"put" | "get">, validUntil: number, extra: object = {}) =>
    Effect.tryPromise(() => issueSignedToken({ ...auth, pathname: path, operations, validUntil, ...extra }))

  return Blob.of({
    uploadTarget: (path, opts) =>
      Effect.gen(function*() {
        const validUntil = Date.now() + opts.validForMs
        // The blob must land at exactly `path`: presigned uploads add a random suffix unless told not to.
        const constraints = {
          allowedContentTypes: [opts.contentType],
          maximumSizeInBytes: opts.maxBytes,
          addRandomSuffix: false
        }
        const token = yield* signed(path, ["put"], validUntil, constraints)
        const { presignedUrl } = yield* Effect.tryPromise(() =>
          presignUrl(token, { operation: "put", pathname: path, access: "private", validUntil, ...constraints })
        )
        return { url: presignedUrl, method: "PUT" as const, headers: { "content-type": opts.contentType }, expiresAt: validUntil }
      }).pipe(Effect.catch(unavailable("upload signing"))),
    size: (path) =>
      Effect.tryPromise({ try: () => head(path, auth), catch: (cause) => cause }).pipe(
        Effect.map((h) => h.size),
        // Nothing uploaded yet is an answer, not an outage.
        Effect.catch((cause) =>
          cause instanceof BlobNotFoundError ? Effect.succeed(undefined) : unavailable("head")(cause)
        )
      ),
    transcriberSource: (path) =>
      Effect.gen(function*() {
        const validUntil = Date.now() + 2 * 60 * 60 * 1000
        const token = yield* signed(path, ["get"], validUntil)
        const { presignedUrl } = yield* Effect.tryPromise(() =>
          presignUrl(token, { operation: "get", pathname: path, access: "private", validUntil, useCache: false })
        )
        return { _tag: "Url" as const, url: presignedUrl }
      }).pipe(Effect.catch(unavailable("read signing"))),
    delete: (paths) =>
      paths.length === 0
        ? Effect.void
        : Effect.tryPromise(() => del([...paths], auth)).pipe(
          Effect.catch((cause) => Effect.logWarning("Blob delete failed", cause))
        )
  })
}

/** HMAC over what a local blob URL allows, keyed by the server secret. */
export const localSignature = (secret: string, method: string, path: string, expires: number, maxBytes: number) =>
  createHmac("sha256", secret).update(`${method}\n${path}\n${expires}\n${maxBytes}`).digest("base64url")

export const verifyLocalSignature = (
  secret: string,
  method: string,
  path: string,
  expires: number,
  maxBytes: number,
  signature: string
) => {
  const expected = Buffer.from(localSignature(secret, method, path, expires, maxBytes))
  const given = Buffer.from(signature)
  return expires > Date.now() && expected.length === given.length && timingSafeEqual(expected, given)
}

/** The file for a blob path, refusing anything that would escape the store's directory. */
export const localFile = (root: string, path: string): string | undefined => {
  const base = resolve(root)
  const file = resolve(base, path)
  return file.startsWith(base + "/") && /^[\w./-]+$/.test(path) ? file : undefined
}

const localBlob = (config: AppConfig["Service"]) => {
  const secret = Redacted.value(config.serverSecret)
  const root = config.blob.localDir
  const url = (method: "PUT" | "GET", path: string, validForMs: number, maxBytes: number) => {
    const expires = Date.now() + validForMs
    const sig = localSignature(secret, method, path, expires, maxBytes)
    return {
      url: `${config.publicUrl}/v1/dev/blob/${path}?expires=${expires}&max=${maxBytes}&sig=${sig}`,
      expires
    }
  }
  return Blob.of({
    uploadTarget: (path, opts) =>
      Effect.sync(() => {
        const { url: u, expires } = url("PUT", path, opts.validForMs, opts.maxBytes)
        return { url: u, method: "PUT" as const, headers: { "content-type": opts.contentType }, expiresAt: expires }
      }),
    size: (path) =>
      Effect.promise(async () => {
        const file = localFile(root, path)
        if (!file) return undefined
        return stat(file).then((s) => s.size, () => undefined)
      }),
    // AssemblyAI can't reach a developer's machine, so the audio is uploaded to it instead.
    transcriberSource: (path) =>
      Effect.sync(() => {
        const file = localFile(root, path)!
        return {
          _tag: "Bytes" as const,
          stream: NodeStream.fromReadable({
            evaluate: () => createReadStream(file),
            onError: () => new UpstreamFailed({ message: "The uploaded audio couldn't be read." })
          })
        }
      }),
    delete: (paths) =>
      Effect.promise(() =>
        Promise.all(paths.map((p) => {
          const file = localFile(root, p)
          return file ? rm(file, { force: true }) : Promise.resolve()
        }))
      ).pipe(Effect.asVoid)
  })
}

/** Ensures a local blob's directory exists before it is written. */
export const prepareLocalFile = (file: string) => mkdir(dirname(file), { recursive: true })
