/**
 * POST /v1/ask: one model round of a question. Validate, reserve in Convex, inject the contract
 * and the pinned model settings, stream the Gateway's events through unchanged, and settle what
 * the round cost once the stream ends (on success, failure or the client going away).
 */
import { Effect, Exit, Stream } from "effect"
import { HttpRouter, HttpServerRequest, HttpServerResponse } from "effect/http"
import type { TokenUsage } from "../../convex/lib/pricing.js"
import { MAX_BODY_BYTES, validateAsk } from "../AskRequest.js"
import { prefixTokens } from "../Contract.js"
import { type ApiError, fromDenial, InvalidRequest, toResponse, TooLarge, Unauthenticated } from "../Errors.js"
import { errorEvent, SseMeter } from "../SseMeter.js"
import { Auth, bearer } from "../services/Auth.js"
import { Background } from "../services/Background.js"
import { Gateway } from "../services/Gateway.js"
import { Store } from "../services/Store.js"

const ZERO: TokenUsage = { inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0 }

export const AskRoute = HttpRouter.use((router) =>
  Effect.gen(function*() {
    const auth = yield* Auth
    const store = yield* Store
    const gateway = yield* Gateway
    const background = yield* Background

    const ask = Effect.fn("ask")(function*(request: HttpServerRequest.HttpServerRequest) {
      const started = Date.now()
      const token = bearer(request.headers.authorization)
      if (!token) return yield* new Unauthenticated({ message: "Sign in to use Ask." })
      const user = yield* auth.verify(token)
      if (Number(request.headers["content-length"] ?? 0) > MAX_BODY_BYTES) {
        return yield* new TooLarge({ message: "This conversation is too long; start a new one." })
      }
      const text = yield* request.text.pipe(
        Effect.mapError(() => new InvalidRequest({ message: "The body couldn't be read." }))
      )
      const valid = yield* validateAsk(text)
      const { body, contract } = valid

      const begin = yield* store.beginRound({
        clerkId: user.clerkId,
        email: user.email,
        questionId: body.questionId,
        round: body.round,
        mode: body.mode,
        estimatedInputTokens: Math.ceil(text.length / 3) + prefixTokens(contract)
      })
      if (!begin.ok) return yield* fromDenial(begin.error)
      const pinned = begin.model
      yield* Effect.annotateCurrentSpan({ model: pinned.gatewayModel, provider: pinned.provider, round: body.round })

      /** Settles the reservation; without usage the reservation itself is charged. */
      const settle = (usage: TokenUsage | undefined, status: string, firstTokenAt?: number) =>
        store
          .settleRound({
            reservationId: begin.reservationId,
            usage,
            status,
            ttftMs: firstTokenAt === undefined ? undefined : firstTokenAt - started,
            durationMs: Date.now() - started
          })
          .pipe(
            Effect.retry({ times: 2 }),
            Effect.asVoid,
            // The 5-minute sweeper charges anything left unsettled.
            Effect.catch(() => Effect.logError("settleRound failed; the sweeper will charge the reservation")),
            Effect.withSpan("ask.settle")
          )

      const upstream = yield* gateway
        .stream({
          model: pinned.gatewayModel,
          max_tokens: pinned.maxTokens,
          stream: true,
          thinking: pinned.thinking === "adaptive" ? { type: "adaptive", display: "summarized" } : { type: "disabled" },
          output_config: { effort: pinned.effort },
          // Caches the tools, the system prompt and the conversation so far.
          cache_control: { type: "ephemeral" },
          system: contract.system,
          tools: contract.tools,
          messages: valid.messages,
          // The question's provider on every round: thinking blocks and caches belong to one
          // provider, so the Gateway must not fail over by itself mid-question.
          providerOptions: { gateway: { only: [pinned.provider], zeroDataRetention: true } }
        })
        .pipe(
          // Nothing was generated: nothing is charged.
          Effect.tapError(() => background.run(settle(ZERO, "upstream_error"))),
          Effect.onInterrupt(() => background.run(settle(undefined, "aborted")))
        )

      const meter = new SseMeter()
      let failed = false
      const events = upstream.body.pipe(
        Stream.tap((chunk) => Effect.sync(() => meter.push(chunk))),
        // After the first byte an error can only travel as an event the client already reads.
        Stream.catch((e) => {
          failed = true
          return Stream.make(errorEvent(e.message))
        }),
        Stream.onExit((exit) => {
          const status = Exit.hasInterrupts(exit)
            ? "aborted"
            : failed
            ? "upstream_error"
            : meter.errored
            ? "error"
            : "ok"
          return background.run(settle(meter.result(), status, meter.firstTokenAt))
        })
      )
      return HttpServerResponse.stream(events, {
        contentType: "text/event-stream",
        headers: {
          "cache-control": "no-cache, no-transform",
          "x-linger-questions-left": String(begin.questionsLeft),
          ...(begin.downgraded ? { "x-linger-downgraded": "1" } : {})
        }
      })
    })

    yield* router.add("POST", "/v1/ask", (request) =>
      ask(request).pipe(Effect.catch((e: ApiError) => Effect.succeed(toResponse(e)))))
  })
)
