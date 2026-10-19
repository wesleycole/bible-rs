/**
 * Handlers for the HttpApi groups in Api.ts: the bearer-token middleware, GET /v1/me, sermons,
 * billing and account deletion.
 */
import { createHash, randomBytes } from "node:crypto"
import { Effect, Layer, Redacted } from "effect"
import { HttpApiBuilder } from "effect/http-api"
import type { Id } from "../../convex/_generated/dataModel.js"
import { Api, Authorization, type Me, type Sermon } from "../Api.js"
import { fromDenial, InvalidRequest, NotFound, Unauthenticated, UpstreamFailed } from "../Errors.js"
import { reconcileSermon } from "../SermonFlow.js"
import { AppConfig } from "../services/AppConfig.js"
import { AppStore } from "../services/AppStore.js"
import { Auth, CurrentUserRef } from "../services/Auth.js"
import { Blob } from "../services/Blob.js"
import { Store } from "../services/Store.js"
import { Stripe } from "../services/Stripe.js"
import { Transcriber } from "../services/Transcriber.js"

export const AuthorizationLive = Layer.effect(
  Authorization,
  Effect.gen(function*() {
    const auth = yield* Auth
    return Authorization.of({
      bearer: Effect.fn(function*(httpEffect, { credential }) {
        const token = Redacted.value(credential)
        if (!token) return yield* new Unauthenticated({ message: "Sign in to use this." })
        const user = yield* auth.verify(token)
        return yield* Effect.provideService(httpEffect, CurrentUserRef, user)
      })
    })
  })
)

export const MeHandlers = HttpApiBuilder.group(
  Api,
  "me",
  Effect.fn(function*(handlers) {
    const store = yield* Store
    return handlers.handle("me", () =>
      Effect.gen(function*() {
        const user = yield* CurrentUserRef
        return (yield* store.me({ clerkId: user.clerkId, email: user.email })) as Me
      }))
  })
)

/** 16 kHz mono 16-bit PCM, as the recorder writes it. */
const WAV_BYTES_PER_SECOND = 32_000

/** A WAV's size must agree with its duration, so the allowance can't be gamed by a short claim. */
export const wavSizeAgrees = (durationSec: number, bytes: number): boolean => {
  const expected = durationSec * WAV_BYTES_PER_SECOND + 44
  return Math.abs(bytes - expected) <= Math.max(64 * 1024, expected * 0.02)
}

/** Whether a vendor on the internet can call this base URL back (not loopback or a private network). */
export const reachable = (base: string): boolean => {
  const host = new URL(base).hostname
  return !(
    host === "localhost" ||
    host.endsWith(".local") ||
    /^(127\.|10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.|0\.0\.0\.0$|\[?::1\]?$)/.test(host)
  )
}

const sha256 = (s: string) => createHash("sha256").update(s).digest("hex")

export const SermonsHandlers = HttpApiBuilder.group(
  Api,
  "sermons",
  Effect.fn(function*(handlers) {
    const store = yield* Store
    const blob = yield* Blob
    const transcriber = yield* Transcriber
    const config = yield* AppConfig
    // The flow's services, so reconciling on read needs nothing from the request.
    const flow = Layer.mergeAll(
      Layer.succeed(Store, store),
      Layer.succeed(Blob, blob),
      Layer.succeed(Transcriber, transcriber)
    )

    return handlers
      .handle("create", ({ payload }) =>
        Effect.gen(function*() {
          const user = yield* CurrentUserRef
          if (!wavSizeAgrees(payload.durationSec, payload.bytes)) {
            return yield* new InvalidRequest({ message: "The file size doesn't match a 16 kHz mono WAV of that length." })
          }
          const created = yield* store.createSermon({
            clerkId: user.clerkId,
            recordingId: payload.recordingId,
            durationSec: payload.durationSec,
            bytes: payload.bytes,
            nonce: randomBytes(12).toString("hex")
          })
          if (!created.ok) return yield* fromDenial(created.error)
          const upload = yield* blob
            .uploadTarget(created.blobPath, { contentType: "audio/wav", maxBytes: payload.bytes, validForMs: 30 * 60 * 1000 })
            .pipe(Effect.tapError(() => store.failSermon({ sermonId: created.sermonId, error: "Upload couldn't be prepared." }).pipe(Effect.ignore)))
          return { sermonId: created.sermonId, upload, trial: created.trial }
        }))
      .handle("transcribe", ({ params }) =>
        Effect.gen(function*() {
          const user = yield* CurrentUserRef
          const sermon = yield* store.getSermon({ clerkId: user.clerkId, sermonId: params.id })
          if (!sermon) return yield* new NotFound({ message: "No such sermon." })
          if (sermon.status === "transcribing" || sermon.status === "done") {
            return { sermonId: sermon.sermonId, status: sermon.status }
          }
          if (sermon.status !== "uploading") {
            return yield* new InvalidRequest({ message: "That sermon can't be transcribed." })
          }
          const size = yield* blob.size(sermon.blobPath)
          if (size === undefined) return yield* new InvalidRequest({ message: "The audio hasn't been uploaded yet." })
          if (size !== sermon.bytes) {
            return yield* new InvalidRequest({ message: `The upload is incomplete (${size} of ${sermon.bytes} bytes).` })
          }
          const secret = randomBytes(24).toString("hex")
          const sermonId = sermon.sermonId as Id<"sermons">
          const assemblyId = yield* blob.transcriberSource(sermon.blobPath).pipe(
            Effect.flatMap((audio) =>
              transcriber.start({
                audio,
                webhook: reachable(config.publicUrl)
                  ? { url: `${config.publicUrl}/v1/webhooks/assemblyai`, secret }
                  : undefined
              })
            ),
            Effect.tapError(() =>
              store.failSermon({ sermonId, error: "Transcription couldn't start." }).pipe(
                Effect.andThen(blob.delete([sermon.blobPath])),
                Effect.ignore
              )
            )
          )
          const started = yield* store.startTranscription({
            clerkId: user.clerkId,
            sermonId: params.id,
            assemblyId,
            webhookSecretHash: sha256(secret)
          })
          if (!started.ok) return yield* fromDenial(started.error)
          return { sermonId: sermon.sermonId, status: "transcribing" as const }
        }))
      .handle("get", ({ params }) =>
        Effect.gen(function*() {
          const user = yield* CurrentUserRef
          const read = store.getSermon({ clerkId: user.clerkId, sermonId: params.id })
          let sermon = yield* read
          if (!sermon) return yield* new NotFound({ message: "No such sermon." })
          if (sermon.status === "transcribing" && (yield* reconcileSermon(sermon).pipe(Effect.provide(flow)))) {
            sermon = (yield* read) ?? sermon
          }
          const out: { -readonly [K in keyof Sermon]: Sermon[K] } = { sermonId: sermon.sermonId, status: sermon.status }
          if (sermon.error) out.error = sermon.error
          if (sermon.status === "done" && sermon.transcriptUrl) {
            const url = sermon.transcriptUrl
            out.transcript = yield* Effect.tryPromise(() => fetch(url).then((r) => r.json())).pipe(
              Effect.mapError(() => new UpstreamFailed({ message: "The transcript couldn't be read." }))
            ) as Effect.Effect<NonNullable<Sermon["transcript"]>, UpstreamFailed>
          }
          return out
        }))
  })
)

export const BillingHandlers = HttpApiBuilder.group(
  Api,
  "billing",
  Effect.fn(function*(handlers) {
    const store = yield* Store
    const appStore = yield* AppStore
    const stripe = yield* Stripe

    return handlers
      .handle("appleTransaction", ({ payload }) =>
        Effect.gen(function*() {
          const user = yield* CurrentUserRef
          const me = (yield* store.me({ clerkId: user.clerkId, email: user.email })) as Me
          const e = yield* appStore.verifyTransaction(payload.jws)
          // A purchase made for another Linger account (shared Apple ID) isn't this user's.
          if (e.appAccountToken && e.appAccountToken.toLowerCase() !== me.user.appAccountToken.toLowerCase()) {
            return yield* new InvalidRequest({ message: "That purchase belongs to another Linger account." })
          }
          yield* store.upsertEntitlement({
            owner: { clerkId: user.clerkId },
            source: "app_store",
            productId: e.productId,
            status: e.status,
            periodStart: e.periodStart,
            expiresAt: e.expiresAt,
            originalId: e.originalId,
            environment: e.environment
          })
          return (yield* store.me({ clerkId: user.clerkId, email: user.email })) as Me
        }))
      .handle("stripeCheckout", ({ payload }) =>
        Effect.gen(function*() {
          const user = yield* CurrentUserRef
          yield* store.me({ clerkId: user.clerkId, email: user.email })
          const profile = yield* store.billingProfile({ clerkId: user.clerkId })
          if (!profile) return yield* new InvalidRequest({ message: "No such user." })
          const customer = yield* stripe.ensureCustomer({
            clerkId: user.clerkId,
            email: profile.email,
            existing: profile.stripeCustomerId
          })
          if (customer !== profile.stripeCustomerId) {
            yield* store.setStripeCustomer({ clerkId: user.clerkId, stripeCustomerId: customer })
          }
          return { url: yield* stripe.checkout({ customer, interval: payload.interval, clientReferenceId: profile.userId }) }
        }))
      .handle("stripePortal", () =>
        Effect.gen(function*() {
          const user = yield* CurrentUserRef
          const profile = yield* store.billingProfile({ clerkId: user.clerkId })
          if (!profile?.stripeCustomerId) {
            return yield* new InvalidRequest({ message: "There's no subscription bought on the web to manage." })
          }
          return { url: yield* stripe.portal(profile.stripeCustomerId) }
        }))
  })
)

/** Deletes what a purged account left with vendors. Best effort: each service logs its own failures. */
export const cleanupVendors = Effect.fn("account.cleanup")(function*(left: {
  readonly blobPaths: ReadonlyArray<string>
  readonly assemblyIds: ReadonlyArray<string>
  readonly stripeCustomerId?: string | undefined
}) {
  const blob = yield* Blob
  const transcriber = yield* Transcriber
  const stripe = yield* Stripe
  yield* blob.delete(left.blobPaths)
  yield* Effect.forEach(left.assemblyIds, (id) => transcriber.delete(id), { concurrency: 4, discard: true })
  if (left.stripeCustomerId) yield* stripe.deleteCustomer(left.stripeCustomerId)
})

export const AccountHandlers = HttpApiBuilder.group(
  Api,
  "account",
  Effect.fn(function*(handlers) {
    const store = yield* Store
    const auth = yield* Auth
    const vendors = Layer.mergeAll(
      Layer.succeed(Blob, yield* Blob),
      Layer.succeed(Transcriber, yield* Transcriber),
      Layer.succeed(Stripe, yield* Stripe)
    )
    return handlers.handle("delete", () =>
      Effect.gen(function*() {
        const user = yield* CurrentUserRef
        const left = yield* store.purgeAccount({ clerkId: user.clerkId })
        yield* cleanupVendors(left).pipe(Effect.provide(vendors))
        yield* auth.deleteUser(user.clerkId)
      }))
  })
)

export const JsonRoutes = HttpApiBuilder.layer(Api).pipe(
  Layer.provide([MeHandlers, SermonsHandlers, BillingHandlers, AccountHandlers]),
  Layer.provide(AuthorizationLive)
)
