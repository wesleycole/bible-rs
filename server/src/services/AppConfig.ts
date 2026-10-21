/**
 * Configuration, read once from the environment. Secrets stay `Redacted` so they never reach a
 * log; optional vendors are `undefined` when unset, and their services answer
 * `upstream_unavailable` instead of failing the whole deployment.
 */
import { Config, Context, Effect, Layer, Option, Redacted } from "effect"

export interface Settings {
  /** The proxy's own base URL, which Clerk's webhook is verified against. */
  readonly publicUrl: string
  readonly convexUrl: string
  readonly serverSecret: Redacted.Redacted<string>
  readonly clerk: {
    readonly secretKey: Redacted.Redacted<string> | undefined
    /** PEM public key, for networkless verification. */
    readonly jwtKey: string | undefined
    readonly issuer: string | undefined
    readonly authorizedParties: ReadonlyArray<string>
    readonly webhookSecret: Redacted.Redacted<string> | undefined
  }
  readonly gateway: {
    readonly url: string
    /** Used only when no Vercel OIDC token is available (local development). */
    readonly apiKey: Redacted.Redacted<string> | undefined
  }
  readonly apple: {
    readonly bundleId: string | undefined
    readonly appAppleId: number | undefined
    readonly environment: "Production" | "Sandbox" | "Xcode" | "LocalTesting"
    readonly issuerId: string | undefined
    readonly keyId: string | undefined
    readonly privateKey: Redacted.Redacted<string> | undefined
    readonly rootCertsDir: string
  }
  readonly stripe: {
    readonly secretKey: Redacted.Redacted<string> | undefined
    readonly webhookSecret: Redacted.Redacted<string> | undefined
    readonly priceMonthly: string | undefined
    readonly priceYearly: string | undefined
    readonly returnUrl: string
  }
}

const optional = <A>(config: Config.Config<A>) => config.pipe(Config.option, Config.map(Option.getOrUndefined))

const optionalSecret = (...names: ReadonlyArray<string>) =>
  names
    .map((name) => Config.Redacted(name))
    .reduce((a, b) => a.pipe(Config.orElse(() => b)))
    .pipe(Config.option, Config.map(Option.getOrUndefined))

/** On Vercel the deployment's own URL; locally PUBLIC_URL or the dev server's. */
const publicUrl = Config.String("PUBLIC_URL").pipe(
  Config.orElse(() => Config.String("VERCEL_PROJECT_PRODUCTION_URL").pipe(Config.map((h) => `https://${h}`))),
  Config.orElse(() => Config.String("VERCEL_URL").pipe(Config.map((h) => `https://${h}`))),
  Config.withDefault("http://127.0.0.1:3000")
)

export class AppConfig extends Context.Service<AppConfig, Settings>()("linger/services/AppConfig") {
  static readonly layer = Layer.effect(
    AppConfig,
    Effect.gen(function*() {
      return AppConfig.of({
        publicUrl: (yield* publicUrl).replace(/\/$/, ""),
        convexUrl: yield* Config.String("CONVEX_URL"),
        serverSecret: yield* Config.Redacted("LINGER_SERVER_SECRET"),
        clerk: {
          secretKey: yield* optionalSecret("CLERK_SECRET_KEY"),
          jwtKey: yield* optional(Config.String("CLERK_JWT_KEY")),
          issuer: yield* optional(Config.String("CLERK_JWT_ISSUER")),
          authorizedParties: (yield* Config.String("CLERK_AUTHORIZED_PARTIES").pipe(Config.withDefault("")))
            .split(",")
            .map((s) => s.trim())
            .filter((s) => s.length > 0),
          webhookSecret: yield* optionalSecret("CLERK_WEBHOOK_SIGNING_SECRET")
        },
        gateway: {
          url: yield* Config.String("AI_GATEWAY_URL").pipe(Config.withDefault("https://ai-gateway.vercel.sh")),
          apiKey: yield* optionalSecret("AI_GATEWAY_API_KEY")
        },
        apple: {
          bundleId: yield* optional(Config.String("APPLE_BUNDLE_ID")),
          appAppleId: yield* optional(Config.Int("APPLE_APP_APPLE_ID")),
          environment: yield* Config.Literals(["Production", "Sandbox", "Xcode", "LocalTesting"], "APPLE_ENVIRONMENT")
            .pipe(Config.withDefault("Sandbox")),
          issuerId: yield* optional(Config.String("APPLE_ISSUER_ID")),
          keyId: yield* optional(Config.String("APPLE_KEY_ID")),
          privateKey: yield* optionalSecret("APPLE_PRIVATE_KEY"),
          rootCertsDir: yield* Config.String("APPLE_ROOT_CERTS_DIR").pipe(Config.withDefault("certs/apple"))
        },
        stripe: {
          secretKey: yield* optionalSecret("STRIPE_SECRET_KEY"),
          webhookSecret: yield* optionalSecret("STRIPE_WEBHOOK_SECRET"),
          priceMonthly: yield* optional(Config.String("STRIPE_PRICE_MONTHLY")),
          priceYearly: yield* optional(Config.String("STRIPE_PRICE_YEARLY")),
          returnUrl: yield* Config.String("STRIPE_RETURN_URL").pipe(
            Config.withDefault("https://trylinger.com/billing/done")
          )
        }
      })
    })
  )

  /** Fixed settings, for tests. */
  static readonly layerTest = (overrides: Partial<Settings> = {}) =>
    Layer.succeed(
      AppConfig,
      AppConfig.of({
        publicUrl: "http://proxy.test",
        convexUrl: "http://convex.test",
        serverSecret: Redacted.make("test-secret"),
        clerk: { secretKey: undefined, jwtKey: undefined, issuer: undefined, authorizedParties: [], webhookSecret: undefined },
        gateway: { url: "http://gateway.test", apiKey: Redacted.make("gateway-key") },
        apple: {
          bundleId: undefined,
          appAppleId: undefined,
          environment: "Sandbox",
          issuerId: undefined,
          keyId: undefined,
          privateKey: undefined,
          rootCertsDir: "certs/apple"
        },
        stripe: {
          secretKey: undefined,
          webhookSecret: undefined,
          priceMonthly: undefined,
          priceYearly: undefined,
          returnUrl: "https://trylinger.com/billing/done"
        },
        ...overrides
      })
    )
}
