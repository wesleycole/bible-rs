# M9: Launch

Status: not started. Depends on: all earlier milestones. Design: [backend.md](backend.md).

**Goal.** Ship Linger as the personal Bible context manager on the App Store, as a notarized Mac
download, and on the web, with the legal, privacy and operational work done.

## Accounts and domains

- **`trylinger.com`** (registered in M1, with `auth.trylinger.com` for Clerk): add `api.trylinger.com` for the proxy,
  `app.trylinger.com` for the web, and the marketing site and privacy page.
- **Clerk** already runs on its production instance (M1). Add passkeys if the plan allows them, and point
  `API_URL` (`api.rs`), the iOS and web configs at `api.trylinger.com`.
- **Convex:** confirm the production deployment's region and plan, and set usage alerts.
- **Vercel:** connect `linger-api` to Git so `main` deploys, and add the web project.

## Legal and privacy

- Answers to every question in [backend.md](backend.md#open-questions): the lawyer on book photos and
  sermon audio (if M4 didn't already have it); the publishers' answers on the online translations (or keep
  them out of release builds).
- The privacy page and App Store privacy labels from [backend.md](backend.md#privacy-and-consent); terms of
  service; the recording-consent wording.

## Operations

- **Observability** ([backend.md](backend.md#observability)): Sentry for the proxy and Convex, OTLP traces,
  a daily spend alert and a Gateway budget alert.
- **The Bedrock drill:** flip `routing` to Bedrock, confirm new questions move and in-flight ones don't,
  flip back.
- **Runbooks** for "Claude down", "Gateway down", "Convex down" (reading works, Ask and sync say they're
  unavailable, captures wait in the outbox) and "AssemblyAI down" (jobs retry; Free on-device
  transcription is unaffected).
- **Eval baselines** recorded for `eval-ask`, `eval-extract` and `eval-transcribe` on the production
  model rows.

## Releases

- **iOS:** App Store submission with review notes on sign-in (the library), account deletion, the
  recording consent and the subscription terms; screenshots of the reader, Ask and capture.
- **macOS:** a notarized release with `scripts/release-macos.sh`, the new entitlements (camera access is
  not needed for Continuity Camera), and release notes.
- **Web:** `app.trylinger.com` live, linked from the site.
- **README and screenshots** describe the library, not "in progress".

## After launch

- After the first month, review `ledger` and `usage` and retune `plans`: questions, photos, storage, and
  whether heavy photo users need a cost ceiling ([backend.md](backend.md#plans-and-limits)).
- Decide on vectors from real misses ([library.md](library.md#vectors-if-the-eval-asks-for-them)).
- Revisit church libraries ([library.md](library.md#later)).

## Done when

Linger is live on all three platforms, every open question in [backend.md](backend.md#open-questions)
has an answer or a decision, and the runbooks have been walked through once.
