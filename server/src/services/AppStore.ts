/**
 * App Store purchases, verified with Apple's own library against Apple's root certificates
 * (certs/apple). Transactions posted by the app and App Store Server Notifications V2 both become
 * one normalized entitlement; the notification history API replays anything the webhook missed.
 */
import {
  AppStoreServerAPIClient,
  Environment,
  type JWSRenewalInfoDecodedPayload,
  type JWSTransactionDecodedPayload,
  NotificationTypeV2,
  SignedDataVerifier,
  Subtype
} from "@apple/app-store-server-library"
import { readdirSync, readFileSync } from "node:fs"
import { join } from "node:path"
import { Context, Effect, Layer, Redacted } from "effect"
import { InvalidRequest, UpstreamUnavailable } from "../Errors.js"
import { AppConfig } from "./AppConfig.js"

export type EntitlementStatus = "active" | "grace" | "billing_retry" | "expired" | "revoked" | "canceled"

export interface AppleEntitlement {
  readonly originalId: string
  readonly productId: string
  readonly appAccountToken: string | undefined
  readonly status: EntitlementStatus
  readonly periodStart: number
  readonly expiresAt: number
  readonly environment: string
}

export interface AppleNotification {
  readonly notificationId: string
  readonly type: string
  /** Absent for notifications that carry no subscription (TEST, consumption requests). */
  readonly entitlement: AppleEntitlement | undefined
}

/**
 * One subscription's state from its latest transaction, renewal info and (for notifications) what
 * just happened to it.
 */
export const entitlementFrom = (
  tx: JWSTransactionDecodedPayload,
  renewal: JWSRenewalInfoDecodedPayload | undefined,
  notification: { readonly type?: string; readonly subtype?: string } | undefined,
  now: number
): AppleEntitlement | undefined => {
  if (!tx.originalTransactionId || !tx.productId) return undefined
  const expiresAt = tx.expiresDate ?? 0
  const graceUntil = renewal?.gracePeriodExpiresDate
  let status: EntitlementStatus
  let until = expiresAt
  if (tx.revocationDate || notification?.type === NotificationTypeV2.REFUND || notification?.type === NotificationTypeV2.REVOKE) {
    status = "revoked"
  } else if (notification?.type === NotificationTypeV2.EXPIRED || notification?.type === NotificationTypeV2.GRACE_PERIOD_EXPIRED) {
    status = "expired"
  } else if (graceUntil && graceUntil > now && expiresAt <= now) {
    status = "grace"
    until = graceUntil
  } else if (notification?.type === NotificationTypeV2.DID_FAIL_TO_RENEW && notification.subtype !== Subtype.GRACE_PERIOD) {
    status = "billing_retry"
  } else {
    status = expiresAt > now ? "active" : "expired"
  }
  return {
    originalId: tx.originalTransactionId,
    productId: tx.productId,
    appAccountToken: tx.appAccountToken,
    status,
    periodStart: tx.purchaseDate ?? now,
    expiresAt: until,
    environment: String(tx.environment ?? "Production")
  }
}

export interface AppStoreService {
  readonly verifyTransaction: (jws: string) => Effect.Effect<AppleEntitlement, InvalidRequest | UpstreamUnavailable>
  readonly verifyNotification: (signedPayload: string) => Effect.Effect<AppleNotification, InvalidRequest | UpstreamUnavailable>
  /** Signed notification payloads sent since `sinceMs`, for the nightly reconcile. */
  readonly history: (sinceMs: number) => Effect.Effect<ReadonlyArray<string>, UpstreamUnavailable>
}

const unconfigured = new UpstreamUnavailable({ message: "App Store purchases aren't configured on this server." })

export class AppStore extends Context.Service<AppStore, AppStoreService>()("linger/services/AppStore") {
  static readonly layer = Layer.effect(
    AppStore,
    Effect.gen(function*() {
      const { apple } = yield* AppConfig
      if (!apple.bundleId) {
        return AppStore.of({
          verifyTransaction: () => Effect.fail(unconfigured),
          verifyNotification: () => Effect.fail(unconfigured),
          history: () => Effect.fail(unconfigured)
        })
      }
      const environment = Environment[apple.environment === "Production" ? "PRODUCTION" : apple.environment === "Sandbox" ? "SANDBOX" : apple.environment === "Xcode" ? "XCODE" : "LOCAL_TESTING"]
      const roots = readdirSync(apple.rootCertsDir)
        .filter((f) => f.endsWith(".cer"))
        .map((f) => readFileSync(join(apple.rootCertsDir, f)))
      const verifier = new SignedDataVerifier(roots, environment === Environment.PRODUCTION, environment, apple.bundleId, apple.appAppleId)
      const api = apple.privateKey && apple.keyId && apple.issuerId
        ? new AppStoreServerAPIClient(Redacted.value(apple.privateKey), apple.keyId, apple.issuerId, apple.bundleId, environment)
        : undefined

      const invalid = (cause: unknown) =>
        Effect.logWarning("App Store verification failed", cause).pipe(
          Effect.andThen(Effect.fail(new InvalidRequest({ message: "That purchase couldn't be verified." })))
        )

      const verifyTransaction = Effect.fn("AppStore.verifyTransaction")(function*(jws: string) {
        const tx = yield* Effect.tryPromise(() => verifier.verifyAndDecodeTransaction(jws)).pipe(Effect.catch(invalid))
        const entitlement = entitlementFrom(tx, undefined, undefined, Date.now())
        if (!entitlement) return yield* new InvalidRequest({ message: "That transaction isn't a subscription." })
        return entitlement
      })

      const verifyNotification = Effect.fn("AppStore.verifyNotification")(function*(signedPayload: string) {
        const n = yield* Effect.tryPromise(() => verifier.verifyAndDecodeNotification(signedPayload)).pipe(Effect.catch(invalid))
        const data = n.data
        const tx = data?.signedTransactionInfo
          ? yield* Effect.tryPromise(() => verifier.verifyAndDecodeTransaction(data.signedTransactionInfo!)).pipe(Effect.catch(invalid))
          : undefined
        const renewal = data?.signedRenewalInfo
          ? yield* Effect.tryPromise(() => verifier.verifyAndDecodeRenewalInfo(data.signedRenewalInfo!)).pipe(Effect.catch(invalid))
          : undefined
        return {
          notificationId: n.notificationUUID ?? signedPayload.slice(-32),
          type: String(n.notificationType),
          entitlement: tx
            ? entitlementFrom(tx, renewal, { type: n.notificationType, subtype: n.subtype }, Date.now())
            : undefined
        }
      })

      const history = Effect.fn("AppStore.history")(function*(sinceMs: number) {
        if (!api) return yield* unconfigured
        const payloads: Array<string> = []
        let token: string | null = null
        do {
          const page = yield* Effect.tryPromise(() =>
            api.getNotificationHistory(token, { startDate: sinceMs, endDate: Date.now() })
          ).pipe(Effect.mapError(() => new UpstreamUnavailable({ message: "The App Store didn't answer." })))
          for (const item of page.notificationHistory ?? []) {
            if (item.signedPayload) payloads.push(item.signedPayload)
          }
          token = page.hasMore ? page.paginationToken ?? null : null
        } while (token)
        return payloads
      })

      return AppStore.of({ verifyTransaction, verifyNotification, history })
    })
  )
}
