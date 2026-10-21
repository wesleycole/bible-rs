/**
 * The proxy as one Effect app: HttpApi for the JSON routes, plain router routes for Ask's event
 * stream and the webhooks (which need the raw body), and the services they share.
 */
import { Effect, Layer } from "effect"
import { HttpRouter, HttpServer, HttpServerResponse } from "effect/http"
import { HttpApiError } from "effect/http-api"
import { bodyOf, ContractUnsupported, InvalidRequest, toResponse } from "./Errors.js"
import { AskRoute } from "./routes/Ask.js"
import { InternalRoutes } from "./routes/Internal.js"
import { JsonRoutes } from "./routes/Json.js"
import { WebhookRoutes } from "./routes/Webhooks.js"
import { AppConfig } from "./services/AppConfig.js"
import { AppStore } from "./services/AppStore.js"
import { Auth } from "./services/Auth.js"
import { Background } from "./services/Background.js"
import { Gateway } from "./services/Gateway.js"
import { Store } from "./services/Store.js"
import { Stripe } from "./services/Stripe.js"

const HealthRoute = HttpRouter.add("GET", "/v1/health", Effect.succeed(HttpServerResponse.jsonUnsafe({ ok: true })))

/**
 * Sermons moved into the library (Convex). Builds from before M1b still call /v1/sermons; for one
 * release these answer 409 so they say "Update Linger" instead of failing oddly. Then they go.
 */
const updateLinger = Effect.succeed(toResponse(new ContractUnsupported({ message: "Update Linger to transcribe sermons." })))
const RetiredSermonRoutes = Layer.mergeAll(
  HttpRouter.add("POST", "/v1/sermons", updateLinger),
  HttpRouter.add("POST", "/v1/sermons/:id/transcribe", updateLinger),
  HttpRouter.add("GET", "/v1/sermons/:id", updateLinger)
)

/**
 * HttpApi answers a request it can't decode with an empty 400; this gives it the error contract's
 * body like every other error.
 */
const RequestErrors = HttpRouter.middleware(
  (effect) =>
    effect.pipe(
      Effect.catchDefect((defect) => {
        const e = defect as HttpApiError.HttpApiSchemaError
        if (HttpApiError.HttpApiSchemaError.is(defect) && e.kind !== "Body" && e.kind !== "ResponseHeaders") {
          return Effect.succeed(
            HttpServerResponse.jsonUnsafe(
              bodyOf(new InvalidRequest({ message: `The request's ${e.kind.toLowerCase()} is invalid: ${e.cause.message}` })),
              { status: 400 }
            )
          )
        }
        return Effect.die(defect)
      })
    ),
  { global: true }
)

export const Routes = Layer.mergeAll(
  JsonRoutes,
  AskRoute,
  WebhookRoutes,
  InternalRoutes,
  RetiredSermonRoutes,
  HealthRoute,
  RequestErrors
)

export const Services = Layer.mergeAll(
  Auth.layer,
  Store.layer,
  Gateway.layer,
  Background.layer,
  AppStore.layer,
  Stripe.layer
).pipe(Layer.provideMerge(AppConfig.layer))

export const App = Routes.pipe(Layer.provide(Services), Layer.provide(HttpServer.layerServices))

export const { handler, dispose } = HttpRouter.toWebHandler(App)
