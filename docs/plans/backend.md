# The backend

Status: phases 1–4 of the original backend plan are built and deployed (October 2026). Remaining
backend work is scheduled in the [roadmap](README.md#roadmap).

This document owns **accounts, the Ask proxy, the Ask contract, model routing, plans and limits,
payments, licensing of Bible text, privacy and consent, observability, testing of the server and
environments**. The personal library (data model, sync, capture, search, outside sources, web) is in
[library.md](library.md).

## Summary

Linger is a native app whose Bible, search and study tools run on the device. Its **personal library**
(notes, highlights, bookmarks, Recents, sermons and photos) lives in **Convex** and syncs to every
device ([library.md](library.md)). The **Vercel proxy** is a thin policy point for Ask: it keeps the
model key off the device, decides what each user may spend, and records what they spent.

| Layer | Choice | Why |
|---|---|---|
| Proxy | TypeScript on **Effect 4**, one **Vercel Function** (Fluid compute, Node 24) | Already paid for; official SDKs for every vendor; Effect gives typed errors, services as layers and testable streaming |
| Model access | **Vercel AI Gateway**, Anthropic Messages endpoint (`/v1/messages`), with **zero data retention** on every request | One string switches models; zero markup; the proxy authenticates with the deployment's OIDC token |
| Sign-in | **Clerk**: the "Linger" application | Native iOS SDK; OAuth with PKCE for the desktop app; JWTs Convex accepts directly |
| State | **Convex**: billing state and the whole library, records and files | Transactional mutations make quota reservation race-free; live queries for sync; file storage; crons and workflows |
| Transcription | **AssemblyAI** Universal-3.5 Pro, called from Convex | Book names as key terms, speaker labels, deletes its copy on request |
| Photo reading and digests | **Claude** through the Gateway, called from Convex | [library.md](library.md#photos) |
| Payments | **StoreKit 2** on iOS; **Stripe Checkout** for the Developer ID Mac app and Linux | Apple requires IAP on iOS; Developer ID builds can't use IAP |
| Business | **Free** and **Pro** at $7.99/month or $69.99/year | Pro sells AI, transcription and library capacity, never Scripture |

Ask's tool loop runs on the device against local translations and the user's library. Each model
round is one proxied request. The proxy injects the system prompt, tools and model settings, so a
signed-in user can't turn Linger's account into a general-purpose Claude endpoint.

```diagram
┌──────────────────────┐  Clerk token   ┌───────────────────────────────┐  OIDC   ┌────────────────────┐
│ Linger app           │ ─────────────▶ │ Vercel Function (Effect 4)    │ ──────▶ │ Vercel AI Gateway  │ ─▶ Anthropic
│ desktop (egui) / iOS │ ◀── SSE ────── │  /v1/ask  /v1/me  /v1/billing │ ◀─SSE── │  /v1/messages, ZDR │    (Bedrock)
│ / web                │                │  /v1/account  /v1/webhooks/*  │         └────────────────────┘
│                      │                └──────────────┬────────────────┘                   ▲
│ tools run locally;   │                               │ server-secret mutations            │ API key, ZDR
│ search_library calls │  Clerk token   ┌──────────────▼────────────────────────────────┐   │
│ Convex directly      │ ─────────────▶ │ Convex: users, plans, usage, ledger,          │ ──┘
│                      │ ◀── live ───── │ entitlements; library records and files;      │ ──▶ AssemblyAI
└──────────────────────┘                │ workflows, crons                              │ ◀── webhook
                                        └───────────────────────────────────────────────┘
                      App Store, Stripe and Clerk webhooks ─▶ the proxy
```

## What's built

| Area | State | Where |
|---|---|---|
| Ask proxy: validation, reservation, injection, pass-through streaming, settlement, sweeper | Built | `server/src/routes/Ask.ts`, `AskRequest.ts`, `SseMeter.ts`, `convex/usage.ts` |
| Rate limits (20 rounds and 10 questions a minute per user) | Built | `convex/rateLimits.ts` |
| Plans, models and routing, seeded; Deep study; ceiling downgrade; provider pinning | Built | `convex/admin.ts`, `convex/usage.ts` |
| Clerk verification of session and OAuth tokens | Built | `server/src/services/Auth.ts` |
| Desktop sign-in: OAuth PKCE with a loopback redirect; refresh token in the Keychain | Built | `crates/core/src/auth.rs`, `apps/desktop/src/sign_in.rs` |
| Account deletion (`DELETE /v1/account`, Clerk webhook purge) | Built | `server/src/routes/Json.ts`, `convex/account.ts` |
| StoreKit verification, App Store notifications, nightly reconcile | Built (server) | `server/src/services/AppStore.ts`, `routes/Webhooks.ts` |
| Stripe Checkout, Portal and webhook; desktop purchase round-trip | Built | `server/src/services/Stripe.ts`, `crates/core/src/api.rs` |
| Sermons through the proxy: Blob upload, AssemblyAI, webhook, trial hour, 10 Pro hours | Built; **moves to Convex in M1b** | `server/src/routes/Json.ts`, `SermonFlow.ts`, `convex/sermons.ts` |
| Ask consent sheet (desktop) | Built; text changes in M1 | `apps/desktop/src/ask_panel.rs` |
| Load test (1,000 concurrent streams against a mock Gateway) | Built | `server/scripts/load-test.ts` |
| Effect inside Convex handlers (`effectQuery` / `effectMutation`, `ConvexCtx` services, shared errors, `Sequence`) | Built for `highlights`; `Allowance` and the rest in M1 | `convex/lib/effect.ts`, `shared/Errors.ts` |
| Record schema generated from Rust (`cargo xtask schema`), with the drift test | Built for `Highlight` and `Bookmark`; the other records in M1 | `crates/core/src/records.rs`, `shared/records.ts`, `convex/lib/canon.json` |
| iOS sign-in, StoreKit purchase, account deletion | Missing: M3 | |
| On-device transcription for Free | Missing: M5 | |
| `cargo xtask eval-ask` | Missing: M2 | |
| Sentry, OTLP traces, spend alerts | Missing: M9 | |
| `ASWebAuthenticationSession` on macOS (loopback works today) | Missing: M6 | `auth.rs` `TODO(macOS)` |
| Production Clerk instance, `trylinger.com` domain | Missing: **M1, before any library data exists** | |

## Licensing shapes the product

Earlier research established these limits. Every plan respects them.

1. **A paid tier makes the whole app "commercial"** under the API.Bible, ESV and NLT terms, even
   though Pro sells no Scripture.
   - API.Bible counts "subscription services that include Scripture" and any project that "supports a revenue model" as commercial.
   - Crossway's free ESV API is for sites that do not "charge for access to any part of the site".
2. **No copyrighted Bible text from these services may reach a model.** API.Bible lists "sending
   copyrighted text to LLMs as prompt context" as prohibited, and bars AI use of its content,
   including Creative Commons texts, without written consent.
3. **Public-domain texts are unrestricted** for display, caching and model prompts. That covers the
   BSB, WEB and KJV.

What follows:

- **Ask reads only public-domain Bible text.** Its tools read a bundled translation (the BSB by
  default, or the WEB or KJV) whatever the reader shows (`chat::ask_translation`). The Ask panel names
  it. Citations open the passage in the reader's own translation, which is display, not AI use.
- **Pro sells Ask, transcription and library capacity.** Reading, translations, notes, highlights and
  the library itself are free for everyone.
- **ESV, NLT, NET and API.Bible.** The online integrations stay out of release builds
  (`LICENSED_ONLINE`) until Crossway, Tyndale, bible.org and API.Bible answer in writing. "Import a file
  you own" stays. If they say no, the paths are a commercial ESV licence and API.Bible Pro ($29/month
  plus per-translation fees), deferred until revenue justifies them.
- **Library material** (photos of books, sermon audio): the user's own, for private study, under the
  rules in [library.md](library.md#licensing-of-library-material).
- **Letters still to send or answer** are tracked in [Open questions](#open-questions).

## Plans and limits

This table is the one place the allowances are written. They live in Convex's `plans` table, not in
code, and are retuned from real usage.

| | Free (signed in) | Pro |
|---|---|---|
| Price | $0 | **$7.99/month** or **$69.99/year** |
| Reading, translations, notes, highlights, bookmarks, plans, history | Yes | Yes |
| Library and sync on every device (notes, Recents, transcripts, photos) | Yes | Yes |
| Ask model | Haiku 5.5 | Sonnet 5.5, plus Opus 5.5 "Deep study" on request |
| Ask allowance | 30 questions/month, at most 10 a day | 150 questions/month; a Deep study question counts as 2 |
| Rounds per question / output tokens per round | 6 / 4,000 | 8 / 8,000 (Deep study: 8 / 12,000) |
| Hidden monthly Ask cost ceiling | $0.25 | $5.00; past it, questions continue on Haiku with a notice |
| Sermon transcripts | On the device (iOS 26 and macOS 26 with SpeechTranscriber; older macOS and Linux with Whisper; none on iOS 17–18), plus a **one-time 1-hour cloud trial** | Cloud (AssemblyAI), **10 sermon-hours/month**; past them, on the device or next month |
| Sermon audio | Never uploaded, except the trial sermon's, which is deleted after transcription; it stays on the device that recorded it | Kept in the library, plays on every device |
| Photos read (handwriting, slides, pages) | **20 a month** | **300 a month** |
| Library storage | **1 GB** | **25 GB** |

A question is one user message plus every tool round it takes. A photo counts when extraction
starts; a failed extraction gives it back. Digests and chunking aren't counted (they're cheap and
part of a sermon or page already paid for).

The `plans` table gains `storageBytes` and `keepAudio` (M1) and `photosPerMonth` (M4); `usage` gains
`photos`;
`ledger.kind` gains `extract` and `digest` beside `ask` and `sermon`.

**What it costs us.** Ask, at Anthropic list prices through the Gateway with a warm cache and a
typical four-call question:

| Model | In / out per MTok | Typical question | With ~1,200 thinking tokens |
|---|---|---|---|
| Haiku 5.5 | $0.10 / $0.50 | ~$0.001 | ~$0.002 |
| Sonnet 5.5 | $2 / $10 | ~$0.020 | ~$0.032 |
| Opus 5.5 | $4 / $20 | ~$0.039 | ~$0.063 |

Transcription is about $0.26 an audio hour; the library's own costs are in
[library.md](library.md#what-the-library-costs).

| Pro subscriber | Ask | Sermons | Library | Total | Net after Apple's 15% |
|---|---|---|---|---|---|
| Typical: 40 questions, 4 sermon-hours, 40 photos | ~$1.10 | ~$1.05 | ~$0.25 | **~$2.40** | $6.79 |
| Heavy: 150 questions, 10 sermon-hours, 300 photos | ~$4.80 | ~$2.60 | ~$2.00 | **~$9.40** | $6.79 |
| Pathological: $5 Ask ceiling, 10 hours, 300 marked handwriting photos | $5.00 | $2.60 | ~$4.20 | **~$11.80**, then Haiku | $6.79 |

The heavy cases lose money, which is acceptable at launch if they're rare: watch them in the ledger,
and lower the photo allowance or route photos past a monthly cost to Haiku if they aren't. A Free
user costs at most about $0.50 a month (the $0.25 Ask ceiling plus 20 handwriting photos), plus the
one-time trial hour.

**iOS sign-in.** App Review guideline 5.1.1(v) says apps without significant account-based features
must let people use them without a login. The synced personal library is a significant
account-based feature, so Linger requires sign-in everywhere. M3 checks this in review notes before
submission.

## Accounts

### Clerk

- **Application:** "Linger", separate from other apps, so users, branding and sign-in methods don't
  mix. It runs on Clerk's development instance (`elegant-mink-67.clerk.accounts.dev`) today. **M1
  creates the production instance and the `trylinger.com` domain before any library data exists**: a new
  instance gives every user a new `sub`, which would orphan their library. Accounts already on the
  development instance (testers) are remapped once by email with an admin script.
- **Sign-in methods:** email code and passkeys at launch (the user's choice; Google, Apple and
  password are off on the production instance). Sign in with Apple and Google come later.
- **iOS (M3):** the Clerk iOS SDK (`clerk-ios`) with native Sign in with Apple. Requests carry Clerk's
  short-lived session token (`getToken()`) as `Authorization: Bearer`. Account deletion calls
  `DELETE /v1/account`.
- **Web (M8):** Clerk's React SDK; the session token goes to the proxy and to Convex.
- **Desktop (built):** Clerk as an **OAuth 2.0 provider**, a public client with PKCE, because native
  Sign in with Apple and IAP aren't available to Developer ID builds.
  1. The OAuth application "Linger Desktop": public, PKCE required, scopes
     `openid profile email offline_access`. Development: client id `XDg0tXKW3CgO9rJy` on
     `elegant-mink-67.clerk.accounts.dev` (today's defaults in `auth.rs`). Production: client id
     `jXfTnIvmljaPQhDg` on `https://clerk.trylinger.com`, the defaults from the cut-over (M1). Its
     redirect URIs must allow the loopback `http://127.0.0.1:<any port>/callback`, and
     `http://127.0.0.1:53682/callback` for `scripts/clerk-test-token.ts`.
  2. The app generates `code_verifier`, `code_challenge` (S256) and `state`, and opens Clerk's
     `/oauth/authorize`. Today the redirect is a loopback listener on `127.0.0.1` on every platform;
     M6 moves macOS to `ASWebAuthenticationSession` with an Associated Domains callback
     (`https://auth.trylinger.com/oauth/callback`, macOS 14.4+) and a `linger://oauth/callback` fallback.
  3. On the callback the app checks `state` and posts `code` and `code_verifier` to `/oauth/token`.
     It gets a **JWT access token** (one day) and a **refresh token** (Clerk's don't expire; revoked
     on sign-out).
  4. The refresh token and the access token with its expiry go in the Keychain as one item
     (`secrets.rs`; a 0600 file on Linux), so a start inside the access token's life makes no
     grant; a grant happens only within a minute of expiry or after a 401, and its answer is
     written before the new token is used, because Clerk rotates the refresh token on every grant
     and a process killed in between would otherwise leave a dead one. The UI and sync threads
     share one session, and a grant runs under a lock across processes (`secrets.lock` in the data
     directory) that is held from reading the store to writing it: the second process finds the
     first one's new pair and makes no grant, which matters because Clerk revokes a refresh
     token's whole family when one is used twice. A refused grant (`invalid_grant`) forgets the
     session, so the app shows the sign-in prompt. Known limit: a process killed between Clerk's
     answer and the write still loses the session, a sub-second window in the last minute of a
     day-long token, with no recovery but signing in again.

### Verification

- **The proxy** accepts both token kinds: Clerk session tokens (iOS, web) and OAuth access tokens
  (desktop, `typ` `at+jwt`). Both are JWTs, verified locally against Clerk's key with
  `@clerk/backend` `verifyToken`, with no network call per request. The `sub` must be a Clerk user id
  (`user_…`), and a session still pending a sign-in step is refused.
- **The verified `sub` is the only user id in the system.**

### Convex auth

Apps call Convex directly for the library ([library.md](library.md#sync)). `convex/auth.config.ts`
has one provider, and Clerk's Convex integration is activated, so session tokens carry `aud: "convex"`
and `ConvexProviderWithClerk` on the web uses them as they are:

```ts
export default {
  providers: [{
    type: "customJwt",
    issuer: process.env.CLERK_JWT_ISSUER,                      // the Clerk Frontend API URL
    jwks: `${process.env.CLERK_JWT_ISSUER}/.well-known/jwks.json`,
    algorithm: "RS256"
  }]
}
```

With no `applicationID`, Convex doesn't check `aud`, so the same provider accepts session tokens and
the desktop's OAuth access tokens, and gives one person the same `tokenIdentifier` on every device.
Convex doesn't reject the `at+jwt` header. Confirmed with real Clerk tokens on a cloud dev deployment
(October 19, 2026; [below](#answered-in-m1)). Library functions also refuse a `sub` that isn't
`user_…`, as the proxy does.

### Deletion

- `DELETE /v1/account` deletes the Clerk user (Backend API) and starts a Convex **purge job**. A
  `user.deleted` Clerk webhook does the same for deletions that start elsewhere.
- The purge deletes billing rows, library rows and every file. A library can exceed one mutation's
  limits (16,000 writes), so the purge is a chain of scheduled mutations, 500 records at a time, over
  every user table (each milestone adds its tables to the chain and to the purge test), that
  ends by deleting the `users` row. AssemblyAI jobs still open are deleted by a Convex action.
- Confirm that Clerk revokes Sign in with Apple tokens on deletion (Apple TN3194); if not, store and
  revoke the token ourselves.

## The proxy

### Why this shape

- **Effect 4.0** shipped on September 30, 2026, with long-term support until September 2029. Many
  former satellite packages live inside `effect` (`effect/http`, `effect/http-api`,
  `effect/observability`). The HTTP modules are `@stability unstable`, so versions are pinned exactly
  and upgraded on purpose. `server/AGENTS.md` asks contributors to read `node_modules/effect/AGENTS.md`
  first.
- **Vercel Functions with Fluid compute** bill active CPU, not time waiting on Claude; one instance
  serves many streams. On Pro the maximum duration is 800 s. The request body limit is 4.5 MB, which
  Ask requests fit in. Files never pass through the proxy.
- Fluid compute gives the deployment an **OIDC token** the Gateway accepts, so the proxy holds no
  Gateway key. Convex holds one for its own short calls ([Environments](#environments-and-configuration)).

### Repository layout

```
server/                          Vercel project root (Root Directory = server)
  package.json                   pnpm; effect and @effect/* pinned exactly
  vercel.json                    Fluid compute, iad1, maxDuration 800, /v1/* → api/index.ts
  api/index.ts                   the one Vercel entry: export default { fetch }
  src/
    Main.ts                      the app: HttpApi for JSON routes + HttpRouter for SSE and webhooks
    Api.ts                       HttpApi definition (groups: me, billing, account)
    routes/
      Ask.ts                     POST /v1/ask
      Json.ts                    GET /v1/me; Apple transaction attach; Stripe checkout and portal;
                                 DELETE /v1/account
      Internal.ts                routes only Convex calls (server secret): Apple reconcile
      Webhooks.ts                /v1/webhooks/{apple,stripe,clerk}
    services/                    AppConfig, Auth, Store (Convex), Gateway, Background, AppStore, Stripe
    contract/ask-v<N>.json       GENERATED from Rust: system prompt, tools, version, digest
    SseMeter.ts  AskRequest.ts  Contract.ts
  shared/                        M1: what the proxy and convex/ both import
    Errors.ts                    the error contract (moved from src/): the one list of error types
    records.ts                   GENERATED from Rust: an Effect Schema and a Convex validator per record
  convex/
    schema.ts  auth.config.ts (M1)  users.ts  usage.ts  entitlements.ts  account.ts  webhooks.ts
    admin.ts  maintenance.ts  crons.ts  rateLimits.ts  convex.config.ts
    sermons.ts                   transcription jobs (moves to the library workflow in M1b)
    library/                     M1+: items, notes, files, sync, allowance, sermons, photos, search, purge
    lib/effect.ts                M1: runs an Effect handler inside a Convex function (below)
    lib/server.ts                serverQuery/serverMutation/serverAction (server secret)
    lib/user.ts                  M1: userQuery/userMutation/userAction (Clerk identity)
    lib/canon.json               GENERATED from Rust: verses per chapter
  test/                          @effect/vitest and convex-test suites; a mock Gateway
```

M1b removes `services/Blob.ts`, `services/Transcriber.ts`, `routes/DevBlob.ts`, `SermonFlow.ts`, the
`/v1/sermons` handlers (the routes answer `409` for one release, so old builds say "Update Linger"),
`/v1/webhooks/assemblyai` and `/v1/internal/cleanup`, as sermons move into Convex. Sermon tests move to convex-test.

### Effect structure

- Every dependency is a `Context.Service` with a live `Layer` and a test `Layer`. Handlers ask for
  services and never construct clients; tests swap fakes in through `test/harness.ts`.
- **JSON endpoints use `effect/http-api`**, with request and response `Schema`s and bearer auth that
  provides `CurrentUser`. Failures are `Schema.TaggedError` classes with HTTP statuses.
- **The SSE route and webhooks use plain `HttpRouter` routes**: the stream passes through byte for
  byte, and webhooks need the raw body to check signatures.
- `HttpRouter.toWebHandler(...)` makes a Web-standard handler, which `api/index.ts` exports.
- `Effect.gen` for handlers, `Effect.fn("name")` for traced steps, `Effect.ensuring` / `Effect.onExit`
  so settlement runs on success, failure and client abort, and `waitUntil` for work after the response.
- **Not `effect/ai` for Ask.** The client parses raw Anthropic events, and thinking blocks with
  signatures and tool-use deltas must round-trip exactly.
- **The same rules hold inside `convex/`** ([Effect in Convex](#effect-in-convex)): a handler is an
  Effect program over services, not a chain of awaits on `ctx`.

### The Ask contract

The system prompt and tool definitions are written in Rust (`crates/core/src/contract.rs`,
`chat::Tools::definitions()`) and published as a contract:

1. `cargo xtask contract` writes `server/src/contract/ask-v<N>.json`: `version`, `system`, `tools`,
   `max_rounds` and a SHA-256 of the canonical bytes. A `linger-core` test fails if the checked-in
   file differs from what the code generates; the server re-checks each digest at load.
2. The client sends **only the conversation**: `{ contract, questionId, round, mode, messages }`.
3. The proxy injects the model, `system`, `tools`, `max_tokens`, `thinking`, `output_config` and
   `cache_control`, byte-identical for every user, so the shared prompt cache stays warm.
4. Changing the prompt or tools bumps the version. **The proxy serves the current version and the one
   before it**; older clients get `409 contract_unsupported` ("Update Linger").

| Version | Ships in | Change |
|---|---|---|
| `ask/3` | built | served for older clients |
| `ask/4` | built (current) | |
| `ask/5` | M2 | `search_library` and `read_item` replace `search_sermons` and `read_sermon`; the library prompt rules and `{{item …}}` citations ([library.md](library.md#the-library-tools-contract-ask5)) |
| `ask/6` | M7 | `commentaries` and `topics` ([M7](m7-outside-sources.md)) |

For local development, `LINGER_AI_URL` plus `LINGER_AI_GATEWAY_KEY` let a developer call the Gateway
directly; the client then builds the full body from the same contract.

### POST /v1/ask

```
POST /v1/ask
Authorization: Bearer <Clerk token>
{ "contract": "ask/4",
  "questionId": "6f1c…",          // UUID, new for each user message, reused for its tool rounds
  "round": 0,                      // 0 for the first call, then 1, 2… as tools return
  "mode": "standard" | "deep",
  "messages": [ … ] }              // Anthropic message objects, exactly as the client keeps them
→ 200 text/event-stream            // Anthropic SSE, passed through unchanged
  X-Linger-Questions-Left: 112
```

1. **Authenticate:** verify the Clerk JWT; `CurrentUser { clerkId }`.
2. **Validate:** a served contract, a UUID `questionId`, `round` under the plan's cap, a body of at
   most 1 MB with at most 200 messages, alternating roles, content blocks limited to `text`,
   `tool_use`, `tool_result`, `thinking` and `redacted_thinking`, and every `tool_use.name` in the
   contract. Anything else is `400 invalid_request`.
3. **Begin** (`usage.beginRound`, one transaction): load the user, plan and month's usage. On round 0
   check the allowance, daily cap and rate limiter, count the question (Deep counts 2 and needs Pro),
   and **pin the model and provider for the whole question**; past the cost ceiling, pin Haiku. On
   later rounds the question must exist, belong to this user and be under the round cap. Reserve the
   round's worst-case cost. Return the pinned settings, or a typed denial (402 / 429).
4. **Build** the upstream body: the contract's system and tools, the client's messages, the pinned
   settings, `stream: true`, top-level `cache_control`, and
   `providerOptions.gateway: { only: [provider], zeroDataRetention: true }`.
5. **Stream:** POST to the Gateway's `/v1/messages` with the OIDC bearer, pipe the body through
   unchanged, and read `message_start.message.usage` and the final `message_delta.usage` alongside.
6. **Settle** (`usage.settleRound`, through `Effect.ensuring` + `waitUntil`): charge actual usage at
   the prices pinned with the question, replace the reservation and append a ledger row. If the client
   disconnected before `message_delta`, charge the reservation.
7. **Errors before the first byte** become JSON errors; an upstream error mid-stream is forwarded as
   the Anthropic `error` event the client already handles.

**CORS (M8):** the web app calls the proxy from the browser, so the proxy allows the web origin
(`https://app.trylinger.com`, plus preview origins in development) on every route the web uses: `/v1/ask`,
`/v1/me`, `/v1/billing/stripe/checkout`, `/v1/billing/stripe/portal` and `DELETE /v1/account`.

### Model routing and switching

A model is a string. Convex's `models` table is read inside the transaction that reserves, so a
switch applies to the next new question everywhere at once, with no deploy:

| key | Used for | Gateway model | thinking | effort | max_tokens |
|---|---|---|---|---|---|
| `free` | Ask, Free | `anthropic/claude-haiku-5.5` | adaptive | low | 4000 |
| `standard` | Ask, Pro | `anthropic/claude-sonnet-5.5` | adaptive | medium | 8000 |
| `deep` | Ask, Deep study | `anthropic/claude-opus-5.5` | adaptive | high | 12000 |
| `extract-handwriting` | Photos of handwriting (M4) | `anthropic/claude-sonnet-5.5` | disabled | | 4000 |
| `extract-print` | Slides and printed pages (M4) | `anthropic/claude-haiku-5.5`, or Sonnet if the M4 eval says so | disabled | | 4000 |
| `digest` | Sermon and page digests (M2) | `anthropic/claude-haiku-5.5` | disabled | | 4000 |

- **Provider pinning.** The default provider list is `["anthropic"]`. A `routing` document holds an
  outage override (for example `["bedrock"]`) for *new* questions only. A question never changes
  provider mid-loop: thinking blocks only verify in the account that produced them, and prompt caches
  are per provider. Every round passes `only: [provider]`.
- **Only models with zero data retention.** Keep out of `models` any model for which Anthropic retains
  content by default (the Fable and Mythos families), and any provider route without ZDR.
- **Other providers' models** are possible through the same endpoint, but Ask depends on Claude's tool
  use and thinking. A non-Claude row is an experiment behind `cargo xtask eval-ask` before it's
  enabled.

### GET /v1/me

Returns `{ user: { id, email? }, plan: "free" | "pro", renewsAt?, source?, appAccountToken,
ask: { questionsLeft, resetsAt, deepAvailable }, sermons: { hoursLeft, trialAvailable },
library: { storageUsed, storageLimit, photosLeft } }` (`library` from M1, `photosLeft` from M4). The app calls it on launch
when signed in, after a purchase and when Ask opens, and shows it in Settings → Account.

### Errors

Every JSON error is `{ "error": { "type", "message", "upgrade"?, "retryAfter"? } }`. The error classes
live in `shared/Errors.ts` (M1): the proxy encodes them as HTTP statuses, and a Convex handler fails
with the same classes, which `lib/effect.ts` turns into a `ConvexError` carrying the same `type`.
`cargo xtask schema` generates core's `ApiError` from the same list, so a type the server can send is
one the apps can match, and a new one can't be added on one side only.

| Status | type | Client behaviour |
|---|---|---|
| 400 | `invalid_request` | Bug: log it, show "Something went wrong" |
| 401 | `unauthenticated` | Refresh the token once, then show sign-in |
| 402 | `allowance_exhausted` | Show the Pro sheet, or the reset date if already Pro |
| 403 | `plan_required` | Deep study, cloud transcripts or kept audio on Free: the Pro sheet |
| 404 | `not_found` | The record was deleted elsewhere: drop it from the cache |
| 409 | `contract_unsupported` | "Update Linger to keep using Ask" |
| 413 | `too_large` | Conversation too long: suggest a new chat. A file over the storage limit: the Pro sheet or "free up space" |
| 429 | `rate_limited` | Wait `retryAfter` seconds |
| 502 / 503 | `upstream_unavailable` | "Ask is unavailable right now"; library work retries in the background |

## State in Convex

- **Two kinds of caller.** The proxy calls `serverQuery` / `serverMutation` / `serverAction`
  functions, which require `LINGER_SERVER_SECRET` (compared in constant time). The apps call `user*`
  functions with their Clerk token ([Convex auth](#convex-auth)). No function accepts both.
- **Transactions.** Mutations are serializable, so `beginRound` checks and increments allowances
  without races. A mutation's own code must finish within 1 s.
- **Rate limits:** `@convex-dev/rate-limiter`, token buckets per user. M4 adds one for photo
  submissions (30 a minute).
- **Durable work:** `@convex-dev/workflow` for transcription, extraction and digests (actions aren't
  retried on their own). Design every action to finish within 10 minutes: Convex's docs give both 10
  and 30.

### Effect in Convex

Convex handlers are Effect programs, so the library's rules get the same services, schemas and typed
errors as the proxy. `convex/lib/effect.ts` (M1) provides:

- **`ConvexCtx`**, a service holding the handler's `ctx` (`db`, `storage`, `scheduler`, `auth`; in
  actions `runQuery`, `runMutation`, `runAction`). Handlers ask for it; nothing reaches `ctx` another
  way.
- **`CurrentUser`**, provided by `userQuery` / `userMutation` / `userAction` from the Clerk identity,
  and **`ServerCaller`** by the `server*` wrappers. A function provides one or the other, never both.
- **`effectQuery`, `effectMutation` and `effectAction`**: the wrappers decode `args` with the generated
  `Schema` from `shared/records.ts`, run the handler with `Effect.runPromise`, and turn a failure from
  `shared/Errors.ts` into a `ConvexError` with that `type`. Anything else is a defect and is rethrown,
  so Convex logs it as the bug it is.
- **The domain services**, each a `Context.Service` with a live layer over `ConvexCtx` and a test layer:
  - `Allowance`: every check, count and reservation (Ask rounds, sermon seconds, photos, storage) and
    the one place `plans`, `usage` and `reservations` are read and written. The proxy's `beginRound`
    and the apps' `photos.submit` both go through it, so there is one definition of "allowed".
  - `Sequence`: the per-user `seq` and the `storageBytes` counter ([library.md](library.md#the-sync-engine)).
  - `Files`, `Anchors`, `Chunks`: upload URLs and attachment, the per-chapter index, and rebuilding
    chunks from an item's text.
- **Pure modules with no `ctx` at all** for chunking, reciprocal-rank fusion, reference checking against
  `canon.json`, anchor splitting and conflict detection, tested without convex-test.

Convex's limits still apply: a mutation's own code finishes within 1 s, and the function bundle has a
size limit. The M1 spike measures a `userMutation` on Effect against today's plain version before the
library is built on it, and the measurement joins CI.

Billing tables (`convex/schema.ts`); the library's tables are in [library.md](library.md#the-data-model):

| Table | Purpose |
|---|---|
| `users` | `clerkId`, `email?`, `createdAt`, `appAccountToken` (UUID for StoreKit), `stripeCustomerId?`, `trialHourUsed` |
| `plans` | Allowances per plan ([Plans and limits](#plans-and-limits)) |
| `models` | Model rows ([routing](#model-routing-and-switching)) |
| `routing` | Singleton provider override |
| `entitlements` | `source` (`app_store`, `stripe`, `grant`), `productId`, `status`, `periodStart`, `expiresAt`, `originalId`, `environment` |
| `usage` | Per user and month: questions, deep questions, cost, sermon seconds, photos; today's questions |
| `questions` | Pinned settings per Ask question, kept 7 days |
| `reservations` | Open Ask and sermon reservations (photos are counted, not reserved) |
| `ledger` | One row per charged event: `kind` (`ask`, `sermon`, `extract`, `digest`), model, provider, tokens, audio seconds, cost, timings, status. Metadata only, never content. Kept 13 months |
| `sermons` | Transcription jobs ([library.md](library.md#sermons)) |
| `webhookEvents` | Webhook idempotency, kept 30 days |

The effective plan is Pro when any `entitlements` row is active (`active` or `grace`, with `expiresAt`
in the future); otherwise Free. `grant` rows cover comps, press and family (`admin.grant`).

Crons (`convex/crons.ts`):
- Every 5 minutes: sweep reservations older than 15 minutes and charge them.
- Hourly: fail transcription jobs stuck over 3 hours and clean up their vendor state; check on jobs
  whose webhook is late, and finish recordings whose device died (M1b).
- Daily: reconcile App Store subscriptions; prune `questions` (7 days), `ledger` (13 months) and
  `webhookEvents` (30 days); hard-delete library tombstones older than 30 days with their files (M1).
- M1b removes the 24-hour transcript expiry: transcripts are library files now.

## Payments and entitlements

One user can be Pro from either source, and the plan is the same everywhere.

### iOS: StoreKit 2

- **Store setup:** one subscription group, "Linger Pro", with `pro.monthly` ($7.99) and `pro.yearly`
  ($69.99). Enroll in the Small Business Program for 15%.
- **Purchase (M3):** with `appAccountToken = users.appAccountToken` from `/v1/me`, then post
  `Transaction.jwsRepresentation` to `POST /v1/billing/apple/transactions`. The server verifies it with
  `@apple/app-store-server-library` and upserts the entitlement (built).
- **Lifecycle (built):** App Store Server Notifications V2 at `/v1/webhooks/apple`, verified,
  idempotent by `notificationUUID`; the nightly cron replays anything missed.
- **Restore (M3):** re-post `Transaction.currentEntitlements`.
- **Web link (US only):** optional; don't build the economics on it.

### Desktop and Linux: Stripe (built)

- `POST /v1/billing/stripe/checkout` creates a Checkout Session for the same two prices and returns its
  URL; the customer is looked up or created by `clerkId`. The app opens it in the browser.
- `POST /v1/billing/stripe/portal` for managing the subscription.
- `/v1/webhooks/stripe` (signature checked on the raw body) handles `checkout.session.completed`,
  `customer.subscription.created/updated/deleted`, `invoice.paid` and `invoice.payment_failed`, re-reads
  the subscription and upserts. `past_due` gets 14 days' grace.
- Consider **Stripe Managed Payments** (merchant of record, VAT and sales tax, about 3.5% more).

## Privacy and consent

This section is the one place Linger's privacy promises are written; the privacy page, App Store
privacy labels and consent sheets follow it.

- **What's stored.** The library (notes, highlights, bookmarks, Recents, reading position and plan
  progress, sermon transcripts and kept audio, photos and their marks) is stored in Linger's Convex
  deployment (US) so it's on every device. Billing state and a metadata-only ledger are stored there
  too. Bible text and translations stay on the device.
- **Every model call is zero-retention and never used for training.** Ask, photo reading and digests go
  through the Gateway with `zeroDataRetention: true`, only to models and providers that support it.
  AssemblyAI deletes its job (audio and transcript) when the transcript is stored. **Its written
  no-training commitment is a condition of shipping M1b**; if AssemblyAI won't give one, cloud
  transcription moves to a provider that will before M1b ships.
- **Consent sheets.**
  - **Library (M1, at first sign-in after the update, and in onboarding):** your library is stored in
    Linger's cloud so it's on all your devices; photos you add are read by Claude, and recordings
    transcribed by AssemblyAI; neither keeps them or trains on them.
  - **Ask (built; text changes in M1):** questions, the passages Ask reads and the library items it
    finds are sent to Claude through Vercel; nothing is kept for training. It no longer says
    conversations stay on the device.
  - **Recording:** the first recording asks the user to confirm they may record the speaker.
- **Encryption** at rest is Convex's. There's no end-to-end encryption: searching and reading photos
  need the text on the server. The privacy page says so.
- **File links** are unguessable bearer URLs, given only to the owner's apps and to AssemblyAI, and
  never logged ([library.md](library.md#files)).
- **Deletion and export.** Deleting an item deletes its rows and files (after the 30-day tombstone).
  Deleting the account purges everything ([Deletion](#deletion)). Export (M2) writes Markdown for every
  item with its references and sources, plus the original photos and audio, as one zip; the JSON
  export of highlights and bookmarks stays.
- **Logs and traces** carry ids, kinds, sizes, token counts, timings and a hashed user id, never
  bodies. `Authorization` is redacted and Vercel's request-body logging is off.
- **Settings → Account** shows the plan, allowances and storage, Manage subscription, Export, Sign out
  and Delete account.

## Observability

- **Vercel Observability** for function latency and errors; the **AI Gateway dashboard** for spend,
  tokens and errors per model.
- **Convex dashboard and logs** for library functions and workflows; the `ledger` and `usage` tables
  for business numbers (questions a day, photos a day, cost per user, ceiling hits).
- **Effect tracing (M9):** OTLP spans to Grafana Cloud or Axiom: `ask.begin`, `gateway.stream` (time
  to first token and duration), `ask.settle`, `webhook.*`.
- **Errors (M9):** Sentry for unhandled defects in the proxy; Convex exception reporting to the same
  project.
- **Spend (M9):** a daily alert when model spend passes a threshold, and a Gateway budget alert as a
  backstop. Allowances remain the real control.

## Testing

- **Proxy** (`@effect/vitest`, fakes through `test/harness.ts`, built): validation, pinning across a
  routing flip, allowances (30th/31st question, Deep counts 2, ceiling downgrade), settlement on abort
  and on asymmetric usage, webhooks, account deletion.
- **Convex** (`convex-test`, run in-process by the same suites): concurrent `beginRound` at the
  boundary never over-admits; idempotent entitlement upserts; webhook dedupe. M1 adds the library:
  `canRead` refuses another user's records, the change feed returns tombstones and respects cursors,
  outbox mutations are idempotent by `uid`, a stale `baseSeq` makes a conflict copy, and the purge
  chain deletes everything. M1b adds the sermon workflow settling real durations and the join action
  being byte-exact.
- **Contract drift:** the `linger-core` test, and the server's digest check at load.
- **Load:** 1,000 concurrent Ask streams against a mock Gateway (`pnpm load-test`), every reservation
  settled. M1 adds a sync load test: 1,000 clients subscribed to the change feed while 100 write.
- **Payments:** Apple's sandbox and StoreKit Testing (M3); Stripe test-mode events (`stripe trigger`).
- **Ask evaluation (M2):** `cargo xtask eval-ask` runs about 50 public-domain questions plus the
  library eval set against a deployment and scores citation validity, library citation recall, tool use
  and length. Run it before changing any Ask model row.
- **Extraction evaluation (M4):** `cargo xtask eval-extract`, 50 photos with hand-checked text, focus
  passages and references. Run it before changing an `extract-*` row.
- **Transcription evaluation (M5):** `cargo xtask eval-transcribe`, word error rate of on-device
  transcription on public-domain recordings.

## Environments and configuration

| | Production | Development |
|---|---|---|
| Vercel | `linger-api` (`https://linger-api.vercel.app`; `api.trylinger.com` from M9), deployed with the Vercel CLI | `pnpm dev` on `127.0.0.1:3000`; preview deployments |
| Clerk | the production instance, from M1 | development instance |
| Convex | project `linger`, deployment `expert-rabbit-665` | a dev deployment per developer (`npx convex dev`), or the anonymous local backend |
| Stripe | live | test |
| App Store | production | sandbox and Xcode StoreKit config |
| AssemblyAI | production key | same key, its own project if possible |

**Vercel environment** (the Gateway needs none; it uses OIDC):
- Clerk: `CLERK_SECRET_KEY`, `CLERK_JWT_KEY` or `CLERK_JWT_ISSUER`, `CLERK_AUTHORIZED_PARTIES`,
  `CLERK_WEBHOOK_SIGNING_SECRET`.
- Convex: `CONVEX_URL`, `LINGER_SERVER_SECRET`.
- Apple: `APPLE_BUNDLE_ID`, `APPLE_APP_APPLE_ID`, `APPLE_ENVIRONMENT`, `APPLE_ISSUER_ID`,
  `APPLE_KEY_ID`, `APPLE_PRIVATE_KEY`.
- Stripe: `STRIPE_SECRET_KEY`, `STRIPE_WEBHOOK_SECRET`, `STRIPE_PRICE_MONTHLY`,
  `STRIPE_PRICE_YEARLY`, `STRIPE_RETURN_URL`.
- Web (M8): `LINGER_WEB_ORIGINS` for CORS.
- Observability (M9): `SENTRY_DSN`, `OTEL_EXPORTER_OTLP_ENDPOINT`.
- Removed in M1b: `ASSEMBLYAI_API_KEY`, `BLOB_DRIVER`, `BLOB_READ_WRITE_TOKEN`, `BLOB_STORE_ID`,
  `LOCAL_BLOB_DIR`.

**Convex environment** (`npx convex env set`):
- `LINGER_SERVER_SECRET`; `LINGER_PROXY_URL` (for the Apple reconcile callback).
- `CLERK_JWT_ISSUER` (M1, for `auth.config.ts`).
- `ASSEMBLYAI_API_KEY` (M1b) and `AI_GATEWAY_API_KEY` (M2, for digests and extraction). These are short,
  non-streaming calls, so a key is simpler than routing each through the proxy.

**The apps ship only public values:** the proxy URL, the Convex URL, Clerk's publishable key and
Frontend API URL, and the OAuth client id (`crates/core/src/auth.rs`, `api.rs`; the Convex URL joins
them in M1).

## Open questions

| Question | Who | Blocks |
|---|---|---|
| Can a paid Linger offer the ESV, NLT, NET and API.Bible integrations, and users' own API.Bible keys? Confirm the letters were sent, and chase answers | Crossway, Tyndale, bible.org, API.Bible | Their return to release builds |
| Is a user's own photo of a copyrighted book, sent to Claude for their private study, acceptable to rely on? And sermon audio? | Lawyer | Any public (non-TestFlight) release of M4's book-page capture |
| Does AssemblyAI commit in writing to no training on customer audio? | AssemblyAI | M1b release |
| Do Clerk passkeys and the production instance need Clerk Pro for this application? | Clerk dashboard | M1 |
| Does core's M4A muxer give AVPlayer, Safari, Chrome and `ffprobe` the exact duration? | M1b spike | M1b |
| Does Clerk revoke Sign in with Apple tokens on user deletion? | Clerk docs or support | M3 |

### Answered in M1

Measured October 19, 2026 on the cloud dev deployment `adorable-porcupine-835` against Clerk's
development instance, with the test user `amp+clerk_test@example.com` (test mode, code 424242).
`pnpm clerk-test-token` (server) mints both kinds of token without a browser; `cargo run -p
linger-core --features convex --example convex_tokens` reruns the checks through the `convex` crate.

- **One `tokenIdentifier` for both kinds of token: yes.** A desktop OAuth access token (`typ: at+jwt`,
  `client_id` "Linger Desktop", a day long, no `aud`) and a Clerk session token from the hosted
  sign-in flow (60 s, no `aud`, since Clerk's Convex integration isn't activated on the development
  instance) both resolve to `https://elegant-mink-67.clerk.accounts.dev|user_…` in `whoami`. The
  session token came from Clerk's Frontend API, the flow a phone's hosted sign-in uses; an iPhone
  build repeats it in M3.
- **`set_auth_callback` refetches on reconnect: yes.** The crate calls the fetcher once with
  `force_refresh=false`, then with `true` after every reconnect. Convex does not drop a socket or
  error a subscription when its token passes `exp`: a `whoami` subscription stayed live for 150 s on
  a 60 s token. The first request after expiry gets an `AuthError`; the client restarts the protocol,
  calls the fetcher (`true`), and the write then succeeds, about 5 s late.
- **`set_auth` replaces a token before it expires: yes.** Swapping in a fresh session token every
  30 s kept a subscription running through three token lifetimes; each swap re-ran it under the new
  identity, with no error. With one fixed token and no swap, a write after expiry never gets an
  answer (still none after 4 minutes): the client reconnects with the same expired token
  indefinitely. So the sync engine uses `set_auth_callback` over `auth::TokenSource` (refresh on
  `force_refresh`) and also pushes a fresh token 30 s before `exp`, so writes don't wait for the
  `AuthError` round trip.
- **Effect inside Convex's limits: yes.** `highlights.set`: 24.8 ms median `userExecutionTime`
  against 24.1 ms for the same mutation written plainly; the deployment bundles to 694 KB of the
  32 MiB limit ([M1](m1-library-foundation.md#before-any-code-ships)).
