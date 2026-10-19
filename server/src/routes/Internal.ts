/**
 * Routes only Convex calls (its crons have no vendor credentials), authenticated with the shared
 * server secret: cleaning up after failed sermons, and replaying App Store notifications.
 */
import { timingSafeEqual } from "node:crypto"
import { Effect, Layer, Redacted, Schema } from "effect"
import { HttpRouter, HttpServerResponse } from "effect/http"
import { InvalidRequest, toResponse, Unauthenticated, UpstreamUnavailable } from "../Errors.js"
import { bearer } from "../services/Auth.js"
import { AppConfig } from "../services/AppConfig.js"
import { AppStore } from "../services/AppStore.js"
import { Blob } from "../services/Blob.js"
import { Store } from "../services/Store.js"
import { Stripe } from "../services/Stripe.js"
import { Transcriber } from "../services/Transcriber.js"
import { cleanupVendors } from "./Json.js"
import { applyAppleNotification } from "./Webhooks.js"

const Cleanup = Schema.Struct({ blobPaths: Schema.Array(Schema.String), assemblyIds: Schema.Array(Schema.String) })
const Reconcile = Schema.Struct({ sinceMs: Schema.Number })

export const InternalRoutes = HttpRouter.use((router) =>
  Effect.gen(function*() {
    const config = yield* AppConfig
    const secret = Buffer.from(Redacted.value(config.serverSecret))
    const services = Layer.mergeAll(
      Layer.succeed(Store, yield* Store),
      Layer.succeed(Blob, yield* Blob),
      Layer.succeed(Transcriber, yield* Transcriber),
      Layer.succeed(Stripe, yield* Stripe),
      Layer.succeed(AppStore, yield* AppStore)
    )

    const internal = <A, I>(path: HttpRouter.PathInput, schema: Schema.Codec<A, I>, handle: (body: A) => Effect.Effect<unknown, unknown, Store | Blob | Transcriber | Stripe | AppStore>) =>
      router.add("POST", path, (request) =>
        Effect.gen(function*() {
          const given = Buffer.from(bearer(request.headers.authorization) ?? "")
          if (given.length !== secret.length || !timingSafeEqual(given, secret)) {
            return yield* new Unauthenticated({ message: "Internal route." })
          }
          const body = yield* request.json.pipe(
            Effect.flatMap(Schema.decodeUnknownEffect(schema)),
            Effect.mapError(() => new InvalidRequest({ message: "Unexpected payload." }))
          )
          const result = yield* handle(body).pipe(Effect.provide(services))
          return HttpServerResponse.jsonUnsafe(result ?? { ok: true })
        }).pipe(
          Effect.catch((e: unknown) => {
            const tag = (e as { _tag?: string })._tag
            return tag === "Unauthenticated" || tag === "InvalidRequest"
              ? Effect.succeed(toResponse(e as Unauthenticated | InvalidRequest))
              : Effect.logError(`${path} failed`, e).pipe(
                Effect.as(toResponse(new UpstreamUnavailable({ message: "Internal route failed." })))
              )
          })
        ))

    yield* internal("/v1/internal/cleanup", Cleanup, (body) => cleanupVendors(body).pipe(Effect.as({ ok: true })))

    yield* internal("/v1/internal/apple/reconcile", Reconcile, ({ sinceMs }) =>
      Effect.gen(function*() {
        // Until the App Store is set up there is nothing to replay; the nightly cron shouldn't fail.
        if (!config.apple.bundleId) return { replayed: 0, skipped: "The App Store isn't configured." }
        const appStore = yield* AppStore
        const payloads = yield* appStore.history(sinceMs)
        let applied = 0
        for (const payload of payloads) {
          const n = yield* appStore.verifyNotification(payload).pipe(Effect.option)
          if (n._tag === "Some") {
            yield* applyAppleNotification(n.value)
            applied++
          }
        }
        return { replayed: applied }
      }))
  })
)
