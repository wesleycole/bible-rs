/**
 * The Vercel AI Gateway's Anthropic Messages endpoint. On Vercel the deployment's OIDC token
 * authenticates it, so there is no key to store; a local developer uses AI_GATEWAY_API_KEY.
 */
import { getVercelOidcToken } from "@vercel/oidc"
import { Context, Effect, Layer, Redacted, Stream } from "effect"
import { FetchHttpClient, HttpClient, HttpClientRequest } from "effect/http"
import { InvalidRequest, RateLimited, UpstreamFailed, UpstreamUnavailable } from "../Errors.js"
import { AppConfig } from "./AppConfig.js"

export type GatewayError = InvalidRequest | RateLimited | UpstreamFailed | UpstreamUnavailable

export interface GatewayStream {
  /** The event stream's bytes, exactly as the Gateway sent them. */
  readonly body: Stream.Stream<Uint8Array, UpstreamFailed>
}

export interface GatewayService {
  /** Opens a streaming Messages request. Fails before the first byte with a mapped error. */
  readonly stream: (body: unknown) => Effect.Effect<GatewayStream, GatewayError>
}

/** An error response from the Gateway, mapped onto the proxy's contract. */
export const mapUpstream = (status: number, text: string): GatewayError => {
  let message = text.slice(0, 300)
  try {
    const parsed = JSON.parse(text) as { error?: { message?: string } }
    message = parsed.error?.message ?? message
  } catch {
    // Not JSON: keep the text.
  }
  if (status === 400 || status === 413 || status === 422) return new InvalidRequest({ message })
  if (status === 429) return new UpstreamUnavailable({ message: "Ask is busy right now; try again in a moment." })
  if (status === 401 || status === 403) return new UpstreamFailed({ message: "Ask is unavailable right now." })
  return new UpstreamUnavailable({ message: "Ask is unavailable right now." })
}

export class Gateway extends Context.Service<Gateway, GatewayService>()("linger/services/Gateway") {
  static readonly layer = Layer.effect(
    Gateway,
    Effect.gen(function*() {
      const { gateway } = yield* AppConfig
      const client = yield* HttpClient.HttpClient
      const onVercel = process.env.VERCEL === "1"

      const auth = Effect.gen(function*() {
        if (onVercel) {
          const token = yield* Effect.tryPromise(() => getVercelOidcToken()).pipe(Effect.option)
          if (token._tag === "Some") return { authorization: `Bearer ${token.value}` }
        }
        if (gateway.apiKey) return { "x-api-key": Redacted.value(gateway.apiKey) }
        return yield* new UpstreamUnavailable({ message: "Ask isn't configured on this server." })
      })

      const stream = Effect.fn("Gateway.stream")(function*(body: unknown) {
        const headers = yield* auth
        const response = yield* HttpClientRequest.post(`${gateway.url}/v1/messages`).pipe(
          HttpClientRequest.setHeaders({
            ...headers,
            "content-type": "application/json",
            "anthropic-version": "2023-06-01",
            accept: "text/event-stream"
          }),
          HttpClientRequest.bodyJsonUnsafe(body),
          client.execute,
          Effect.mapError(() => new UpstreamUnavailable({ message: "Ask is unavailable right now." }))
        )
        if (response.status !== 200) {
          const text = yield* response.text.pipe(Effect.orElseSucceed(() => ""))
          yield* Effect.logWarning("gateway error", { status: response.status })
          return yield* mapUpstream(response.status, text)
        }
        return {
          body: response.stream.pipe(
            Stream.mapError(() => new UpstreamFailed({ message: "The answer stopped arriving." }))
          )
        }
      })

      return Gateway.of({ stream })
    })
  ).pipe(Layer.provide(FetchHttpClient.layer))
}
