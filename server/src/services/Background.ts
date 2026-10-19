/**
 * Work that outlives the response, such as settling a round after its last byte. On Vercel it is
 * handed to `waitUntil`, so the instance isn't frozen before it finishes; tests run it inline.
 */
import { waitUntil } from "@vercel/functions"
import { Context, Effect, Layer } from "effect"

export interface BackgroundService {
  readonly run: (effect: Effect.Effect<void>) => Effect.Effect<void>
}

export class Background extends Context.Service<Background, BackgroundService>()("linger/services/Background") {
  static readonly layer = Layer.succeed(
    Background,
    Background.of({
      run: (effect) =>
        Effect.sync(() => {
          const promise = Effect.runPromise(effect)
          // Outside Vercel there is no request context to extend; the promise simply runs.
          try {
            waitUntil(promise)
          } catch {
            // Not on Vercel.
          }
        })
    })
  )

  /** Runs the work before returning, so a test sees its effects immediately. */
  static readonly layerTest = Layer.succeed(Background, Background.of({ run: (effect) => effect }))
}
