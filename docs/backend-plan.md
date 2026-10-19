# Linger backend plan

Status: draft, October 19, 2026. This plan replaces the "Rust on Fly with own auth" recommendation from
the earlier proxy research, now that the deciding facts are settled: Linger already pays for **Clerk**,
**Vercel Pro** and **Convex**, wants a gateway for **easy model switching**, and will be sold as
**Free / Pro**.

## Summary

Linger stays a local-first app. Reading, notes, highlights, chat history and sermon transcripts live
on the device, as they do now. The backend is a thin **policy point** with three jobs: keep the
Claude and AssemblyAI keys off the device, decide what each user may spend, and record what they
spent.

| Layer | Choice | Why |
|---|---|---|
| Proxy | TypeScript on **Effect 4**, deployed as **Vercel Functions** (Fluid compute, Node 24) | Already paid for; official SDKs for every vendor; Effect gives typed errors, services as layers and testable streaming |
| Model access | **Vercel AI Gateway**, Anthropic Messages endpoint (`/v1/messages`), authenticated by the deployment's **OIDC token** | One string switches models; no key to store or rotate; zero markup |
| Sign-in | **Clerk**, a new application next to the existing one | Already paid for; native iOS SDK; OAuth with PKCE for the desktop app |
| State | **Convex** | Already paid for; transactional mutations make quota reservation race-free; crons, a rate limiter component and a live dashboard to change models without a deploy |
| Sermon audio | **Vercel Blob**, private store, signed upload and read URLs | The function's 4.5 MB body limit rules out uploading through the proxy |
| Transcription | **AssemblyAI** Universal-3.5 Pro, as the app already uses | Book names as key terms, speaker labels, deletes audio on request |
| Payments | **StoreKit 2** on iOS; **Stripe Checkout** for the Developer ID Mac app and Linux | Apple requires IAP on iOS; Developer ID builds cannot use IAP |
| Business | **Free** (Haiku, monthly allowance, on-device transcription) and **Pro** at $7.99/month or $69.99/year (Sonnet, Opus "deep study", cloud transcripts) | Pro sells AI and transcription, never Scripture |

Ask and transcription keep their current shape: the tool loop and its tools run on the device
against local translations and transcripts. Each model round is one proxied request. The server
injects the system prompt, tools and model settings, so a signed-in user can't turn Linger's account
into a general-purpose Claude endpoint.

```diagram
┌──────────────────────┐  Clerk token   ┌───────────────────────────────┐  OIDC   ┌────────────────────┐
│ Linger app           │ ─────────────▶ │ Vercel Functions (Effect 4)   │ ──────▶ │ Vercel AI Gateway  │ ─▶ Anthropic
│ desktop (egui) / iOS │ ◀── SSE ────── │  /v1/ask  /v1/me  /v1/sermons │ ◀─SSE── │  /v1/messages      │    (Bedrock, Vertex)
│                      │                │  /v1/billing  /v1/webhooks/*  │         └────────────────────┘
│ tools run locally:   │                └──────┬──────────────┬─────────┘
│ get_passage,         │                       │ mutations    │ signed URLs ┌──────────────┐
│ search_bible,        │                       ▼              └───────────▶ │ Vercel Blob  │ ◀─ AssemblyAI
│ search_sermons, ...  │                ┌──────────────┐                    │ (private)    │    fetches audio
└──────────┬───────────┘                │ Convex       │                    └──────────────┘
           │ PUT audio (signed URL)     │ users, usage,│ ◀── webhooks: App Store, Stripe, Clerk, AssemblyAI
           └──────────────────────────▶ │ entitlements │
                       (to Blob)        └──────────────┘
```

## Where things stand

The repo has these today, and the plan builds on them:

- `crates/core/src/assistant.rs` runs the Ask tool loop against `https://ai-gateway.vercel.sh/v1/messages`.
  - The key comes from `LINGER_AI_GATEWAY_KEY` or `AI_GATEWAY_API_KEY`, and the model is `anthropic/claude-opus-5.5`.
  - Each request has `max_tokens: 16000`, adaptive thinking, `effort: medium`, `fallbacks: "default"` and a top-level `cache_control`.
  - Each round is a separate request, with up to `MAX_ROUNDS = 8` per question.
  - Without a key, Ask falls back to `chat::local_reply`.
- `crates/core/src/chat.rs` defines the tools `search_bible`, `get_passage`, `search_sermons` and `read_sermon`, and builds them from the local library and transcripts.
- `crates/core/src/transcribe.rs` handles transcription.
  - It joins a recording's 30-second, 16 kHz mono WAV chunks into one file.
  - It uploads that file to AssemblyAI's `/v2/upload` with a key from `LINGER_ASSEMBLYAI_KEY`.
  - It runs Universal-3.5 Pro with speaker labels and the 66 book names as `keyterms_prompt`, and polls the job.
  - It saves `transcript.json` beside the recording, then deletes the job.
- Ask conversations are saved in `study.sqlite` (the `chats` table) and listed under Recents.
- The macOS app ships as a notarized Developer ID build (see `RELEASING.md`), and an iOS app is in progress.

Both keys come from environment variables, which works on a developer machine and nowhere else.
Phase 1 replaces them.

## Licensing shapes the product

The earlier research established these limits. Every later section respects them.

1. **A paid tier makes the whole app "commercial"** under the API.Bible, ESV and NLT terms, even
   though Pro sells no Scripture.
   - API.Bible counts "subscription services that include Scripture" and any project that "supports a revenue model" as commercial.
   - Crossway's free ESV API is for sites that do not "charge for access to any part of the site".
2. **No copyrighted Bible text may reach a model.** API.Bible lists "sending copyrighted text to
   LLMs as prompt context" as prohibited. Its terms also bar AI use of copyrighted content, including
   Creative Commons texts, without written consent.
3. **Public-domain texts are unrestricted** for display, caching and LLM prompts. That covers the
   BSB, WEB and KJV.

What follows for the product:

- **Ask reads only public-domain text.**
  - Ask's tools always read from a bundled public-domain translation: the BSB by default, or the WEB or KJV if the user prefers. They never read whatever translation the reader has open.
  - The Ask panel says which translation it quotes. Citations still open the passage in the reader's own translation, which is display, not AI use.
  - This gate lives in the client (`chat::Tools` construction in `ask_panel.rs`), because the proxy cannot tell where text came from.
- **Pro sells Ask and transcription.** Reading, translations, notes and highlights are free for everyone.
- **ESV, NLT and API.Bible keys.**
  - The one-click integrations, and users' own API.Bible Starter keys, stay out of release builds until Crossway, Tyndale and API.Bible answer in writing.
  - "Import a module you own" stays available.
  - If the answers come back no, the paths are a commercial ESV license held by Fabrika LLC, and API.Bible Pro at $29/month plus per-translation fees. Both are deferred until revenue justifies them.
- **Sermons.**
  - The audio and the preaching belong to the church. Transcripts already reach Claude through `search_sermons` and `read_sermon`.
  - The consent sheet names both vendors, and the user confirms they may record. A church plan (see Later) settles rights properly.
- **Emails to send in phase 0:**
  - **API.Bible** (support@api.bible): may a paid app keep users' own Starter keys, and FUMS for cached and native views.
  - **Crossway** (licensing@crossway.org): ESV in an app that sells unrelated AI features.
  - **Tyndale:** the same question for the NLT.

## Business model: Free and Pro

Allowances are counted in **questions**, the unit users understand and the one App Review rule
3.1.2(c) expects. A question is one user message plus every tool round it takes. Behind each
allowance sits a hidden **monthly cost ceiling**, so a pathological user can't cost more than the
plan earns.

| | Free (signed in) | Pro |
|---|---|---|
| Price | $0 | **$7.99/month** or **$69.99/year** |
| Reading, translations, notes, highlights, plans, history | Yes | Yes |
| Ask model | Haiku 5.5 | Sonnet 5.5, plus Opus 5.5 "Deep study" on request |
| Ask allowance | 30 questions/month, at most 10 a day | 150 questions/month; a Deep study question counts as 2 |
| Rounds per question / output tokens per round | 6 / 4,000 | 8 / 8,000 (Deep study: 8 / 12,000) |
| Hidden monthly cost ceiling | $0.25 | $5.00. Past it, questions continue on Haiku with a notice instead of stopping |
| Sermon transcripts | On-device (Apple SpeechTranscriber where available), plus a **one-time 1-hour cloud trial** | Cloud transcripts (AssemblyAI) up to **10 sermon-hours/month** |
| Sermon search in Ask | Yes, over any transcript | Yes |
| Recents | Local | Local (sync is a later Pro feature) |

Reading never needs an account. Ask and cloud transcription need sign-in, because the allowance
belongs to an account. App Review guideline 5.1.1(v) allows this for account-based features, and
account deletion must be in the app.

### What it costs us

Per-question costs come from Anthropic list prices through the Gateway (zero markup). They assume a
warm prompt cache and a typical four-call question with a 6k-token stable prefix, about 3k tokens of
tool results and about 800 output tokens:

| Model | In / out per MTok | Typical question | With ~1,200 thinking tokens |
|---|---|---|---|
| Haiku 5.5 | $0.10 / $0.50 | ~$0.001 | ~$0.002 |
| Sonnet 5.5 | $2 / $10 | ~$0.020 | ~$0.032 |
| Opus 5.5 | $4 / $20 | ~$0.039 | ~$0.063 |

Transcription costs about $0.26 per audio hour: $0.21 for Universal-3.5 Pro plus $0.05 for key terms.

| Pro subscriber | Ask | Sermons | Total | Net revenue after Apple's 15% (Small Business Program) |
|---|---|---|---|---|
| Typical: 40 questions, 4 sermon-hours | ~$1.10 | ~$1.05 | **~$2.15** | $6.79 |
| Heavy: 150 questions, 10 sermon-hours | ~$4.80 | ~$2.60 | **~$7.40** | $6.79 |
| Pathological: hits the $5 ceiling, 10 hours | $5.00 | $2.60 | **$7.60**, then Haiku | $6.79 |

A Free user costs at most about $0.06 a month in Haiku. The one-time trial hour costs $0.26 per
account. Fixed infrastructure is already paid, apart from Clerk if the new application needs Pro.
Annual plans are better on Stripe, because the fixed per-charge fee applies once.

All limits live in Convex's `plans` table, not in code. They can be retuned from the Convex
dashboard after the first month of real usage.

## Accounts with Clerk

**Setup.**
- Create a **new Clerk application, "Linger"**, separate from the other app, so users, branding and sign-in methods don't mix. Clerk bills per application.
- Start on Hobby. Move this application to Pro only if it needs a Pro feature (for example passkeys, or custom session lifetimes).
- Confirm in the dashboard whether OAuth applications and the Account Portal need Pro.

**Sign-in methods:** Sign in with Apple, Google, and email code. Add passkeys if the plan allows them.

**iOS.**
- Use the Clerk iOS SDK (`clerk-ios`), with native Sign in with Apple.
- The app sends Clerk's short-lived **session token** (`getToken()`) as `Authorization: Bearer` on each request.
- Account deletion calls `DELETE /v1/account`.

**Developer ID Mac app.** Native Sign in with Apple and IAP aren't available to Developer ID builds,
so the desktop app uses Clerk as an **OAuth 2.0 provider** with a public client and PKCE.
1. In Clerk, create an OAuth application "Linger Desktop".
   - Type: Public, with Require PKCE.
   - Scopes: `openid profile email offline_access`.
   - Redirect URIs:
     - `https://auth.linger.app/oauth/callback`, an Associated Domains `https` callback (macOS 14.4+).
     - `linger://oauth/callback`, a custom-scheme fallback.
     - The loopback `http://127.0.0.1/callback` for Linux. Verify that Clerk accepts any loopback port, as RFC 8252 allows.
2. The app generates `code_verifier`, `code_challenge` (S256) and `state`. On macOS it opens
   `ASWebAuthenticationSession` (via `objc2-authentication-services`) at Clerk's
   `/oauth/authorize`, which shows Clerk's hosted sign-in.
3. On the callback, the app checks `state`, then posts `code` and `code_verifier` to Clerk's
   `/oauth/token`.
   - It gets a **JWT access token** (1-day lifetime) and a **refresh token** (Clerk's refresh tokens don't expire; revoke them on sign-out).
4. The refresh token goes into the OS credential store through the `keyring` crate already in the
   workspace. The access token stays in memory and is refreshed on a 401 or shortly before it
   expires.
5. On Linux the app opens the system browser with a loopback redirect instead of
   `ASWebAuthenticationSession`. The flow is otherwise identical.

**Verification on the server.**
- The proxy accepts both kinds of token: Clerk session tokens from iOS and OAuth access tokens from the desktop.
- Both are JWTs, verified locally against Clerk's JWKS, with no network call per request.
- Use `@clerk/backend` (`authenticateRequest` with both token types accepted, or `verifyToken` plus an issuer and audience check). Pin the exact API names against the SDK version installed.
- The verified `sub` (Clerk user id) is the only user id in the system.

**Deletion.**
- `DELETE /v1/account` deletes the Clerk user (Backend API) and purges the user's Convex documents and any Blob objects. A `user.deleted` Clerk webhook does the same, for deletions that happen elsewhere.
- Confirm that Clerk revokes Sign in with Apple tokens on deletion (Apple TN3194). If it doesn't, store and revoke the token ourselves.

## The proxy: TypeScript on Effect 4, on Vercel

### Why this shape

- **Effect 4.0** shipped on September 30, 2026. It is at 4.0.5 on npm, with a dependency-free core.
  - It has long-term support: bug fixes until September 2029, or one year after 5.0 ships, whichever is later.
  - Many former satellite packages now live inside `effect`: `effect/http`, `effect/http-api`, `effect/ai`, `effect/observability`.
  - Satellite packages such as `@effect/platform-node` and `@effect/vitest` release in lockstep.
  - The HTTP modules are marked `@stability unstable`, so pin exact versions and upgrade on purpose, not by caret.
- **Vercel Functions with Fluid compute** bill active CPU, not time spent waiting on Claude. One instance serves many concurrent streams.
  - On Pro, the default duration is 300 s and the maximum 800 s. The request body limit is 4.5 MB, which Ask requests fit in and sermon audio does not (it goes to Blob).
  - Fluid compute also gives the deployment an **OIDC token** the AI Gateway accepts, so there is no Gateway key at all.

### Repository layout

The backend lives in this repo, so the contract it enforces is generated from the same commit as the
client.

```
server/                          Vercel project root (Root Directory = server)
  package.json                   pnpm; effect, @effect/platform-node, @effect/vitest pinned exactly
  vercel.json                    functions config: maxDuration, regions (iad1), fluid
  api/[[...path]].ts             the one Vercel entry: export default { fetch } from the Effect app
  src/
    Main.ts                      builds the app: HttpApi for JSON routes + HttpRouter for SSE/webhooks
    Api.ts                       HttpApi definition (groups: me, billing, sermons, account)
    routes/
      Ask.ts                     POST /v1/ask: validate, reserve, inject, stream, settle
      Me.ts                      GET /v1/me
      Sermons.ts                 POST /v1/sermons, POST /v1/sermons/:id/transcribe, GET /v1/sermons/:id
      Billing.ts                 Apple transaction attach, Stripe checkout and portal
      Webhooks.ts                /v1/webhooks/{apple,stripe,clerk,assemblyai}
      Account.ts                 DELETE /v1/account
    services/
      AppConfig.ts               Effect Config: env vars, typed and redacted
      Auth.ts                    Clerk verification → CurrentUser
      Store.ts                   Convex client wrapper (every mutation the proxy calls)
      Gateway.ts                 AI Gateway HttpClient, OIDC bearer, SSE metering
      Transcriber.ts             AssemblyAI jobs and deletion
      Blob.ts                    signed PUT/GET URLs, deletion
      AppStore.ts                @apple/app-store-server-library verification
      Stripe.ts                  stripe SDK: checkout, portal, webhook signature
    contract/
      ask-v1.json                GENERATED from Rust: system prompt, tools, version
    Errors.ts                    Schema.TaggedError types and their HTTP mapping
    Pricing.ts                   per-model token prices for cost metering
  convex/
    schema.ts  plans.ts  users.ts  usage.ts  entitlements.ts  sermons.ts  webhooks.ts  crons.ts
    lib/server.ts                customMutation/customQuery that require the server secret
  test/                          @effect/vitest suites with layer fakes; a mock Gateway
```

### Effect structure

- Every dependency is a service (`Context.Service`) with a live `Layer` and a test `Layer`:
  - `AppConfig`, `Auth`, `Store`, `Gateway`, `Transcriber`, `Blob`, `AppStore`, `Stripe`.
  - Handlers ask for services, never construct clients. Tests swap fakes in, without network mocks.
- **JSON endpoints use `effect/http-api`.**
  - `HttpApiEndpoint` with request and response `Schema`s; `HttpApiMiddleware` for auth. The middleware provides `CurrentUser`.
  - Failures are `Schema.TaggedError` classes with HTTP status annotations (see Errors), so the error contract is typed end to end.
- **The SSE route and webhooks use a plain `HttpRouter` route.** The stream must pass through byte for byte, and webhooks need the raw body to check signatures.
- **Entry point.** `HttpRouter.toWebHandler(...)` turns the app into a Web-standard
  `(Request) => Promise<Response>`, which `api/[[...path]].ts` exports. Vercel's Node runtime serves
  Web-standard handlers directly.
- **Coding style.** Use `Effect.gen` for handlers and `Effect.fn("name")` for traced reusable steps.
  Use `Effect.ensuring` / `Effect.onExit` so settlement runs on success, failure and client abort.
  Use `waitUntil` from `@vercel/functions` for work that outlives the response, such as the final
  Convex settle after the last byte.
- **Do not use `effect/ai` or `@effect/ai-anthropic` for Ask.** They normalize provider streams, and
  the client parses raw Anthropic events: thinking blocks with signatures and tool-use deltas must
  round-trip exactly. They are fine for server-only model calls, such as titling or reference
  disambiguation.

### The Ask contract, generated from Rust

The system prompt and tool definitions are written in Rust today (`assistant.rs` `SYSTEM`,
`chat::Tools::definitions()`). They stay there and become a published contract:

1. A new `cargo xtask contract` writes `server/src/contract/ask-v1.json`, containing:
   - `version` (`"ask/1"`), `system`, `tools`, `max_rounds` and a SHA-256 of the canonical bytes.
2. A `linger-core` test fails if the checked-in file differs from what the code generates. CI then
   catches drift in either direction.
3. The client sends **only the conversation**: `{ contract: "ask/1", questionId, round, mode, messages }`.
4. The proxy injects the model, `system`, `tools`, `max_tokens`, `thinking`, `output_config` and
   `cache_control`, byte-identical for every user, so the shared prompt cache stays warm.
5. Changing the prompt or tools bumps the version to `ask/2`. The proxy serves the last two
   versions, and older clients get `409 contract_unsupported` with "Update Linger".

For local development, `LINGER_AI_URL` plus `LINGER_AI_GATEWAY_KEY` still let a developer call the
Gateway directly. In that mode the client builds the full body from the same contract.

### POST /v1/ask

```
POST /v1/ask
Authorization: Bearer <Clerk token>
{ "contract": "ask/1",
  "questionId": "6f1c…",          // UUID, new for each user message, reused for its tool rounds
  "round": 0,                      // 0 for the first call, then 1, 2… as tools return
  "mode": "standard" | "deep",
  "messages": [ … ] }              // Anthropic message objects, exactly as the client keeps them
→ 200 text/event-stream            // Anthropic SSE, passed through unchanged
  X-Linger-Questions-Left: 112
```

The steps:

1. **Authenticate:** verify the Clerk JWT and get `CurrentUser { clerkId }`.
2. **Validate:**
   - **Schema checks:**
     - `contract` is supported, `questionId` is a UUID and `round` < `max_rounds` for the plan.
     - The body is at most 1 MB with at most 200 messages.
     - Roles alternate, and content block types are limited to `text`, `tool_use`, `tool_result`, `thinking` and `redacted_thinking`.
     - Every `tool_use.name` is in the contract.
   - Reject anything else with `400 invalid_request`.
3. **Begin** (one Convex mutation, `usage.beginRound`, transactional):
   - Load the user, their effective plan and this month's usage.
   - On `round == 0`, check the question allowance, the daily cap and the rate limiter, then count the question.
     - Deep study counts 2 and requires Pro.
     - Record `questions[questionId] = { model, provider, startedAt }`. This **pins the model and provider for the whole question**.
   - On a later round, the question must exist, belong to this user and be under the round cap.
   - Past the cost ceiling, downgrade the pinned model to Haiku (at round 0 only).
   - Reserve the round's worst-case cost (`max_tokens` × output price + input estimate).
   - Return `{ model, provider, maxTokens, thinking, effort, reservationId, questionsLeft }`, or a typed denial (402 / 429).
4. **Build** the upstream body: the contract's system and tools, the client's messages, the pinned
   settings, `stream: true`, top-level `cache_control: {type: "ephemeral"}`, and
   `providerOptions: { gateway: { only: [provider] } }`.
5. **Stream:** POST to `https://ai-gateway.vercel.sh/v1/messages` with the OIDC bearer. Pipe the body
   to the client unchanged. Parse the SSE events alongside it to read `message_start.message.usage`
   and the final `message_delta.usage`.
6. **Settle** (Convex mutation `usage.settleRound`, run through `Effect.ensuring` + `waitUntil`):
   - Compute the actual cost from the usage and `Pricing.ts`.
   - Replace the reservation and append a ledger row: model, provider, input, cache-read, cache-write and output tokens, cost, time to first token, duration and status.
   - If the client disconnected before `message_delta`, charge the reservation.
7. **Errors before the first byte** become JSON errors (see Errors). An upstream error after
   streaming started is forwarded as the Anthropic `error` SSE event the client already handles.

The client changes in `assistant.rs`:
- `Config` becomes `{ url, token_source, mode }`.
- It sends the slim body, a fresh `questionId` per user message and `round` per loop iteration.
- It shows 402 / 429 as an upsell or wait message in the panel.
- It drops `fallbacks: "default"`, because the proxy owns routing.

### Model routing and switching

The point of the Gateway is that a model is a string. Convex holds a `models` table that the proxy
reads at round 0, with a 60 s in-memory cache per function instance:

| key | gateway model | thinking | effort | max_tokens | enabled |
|---|---|---|---|---|---|
| `free` | `anthropic/claude-haiku-5.5` | adaptive | low | 4000 | yes |
| `standard` | `anthropic/claude-sonnet-5.5` | adaptive | medium | 8000 | yes |
| `deep` | `anthropic/claude-opus-5.5` | adaptive | high | 12000 | yes |

Plans map to keys: Free → `free`; Pro → `standard`, or `deep` when the user asks for Deep study.

- **Switching a model** is editing one row in the Convex dashboard. New questions pick it up within a minute, with no deploy and no app update. Questions already in flight keep their pinned model.
- **Provider pinning.**
  - The default provider list is `["anthropic"]`.
  - A `routing` document holds an outage override (for example `["bedrock"]`) that applies to *new* questions only.
  - Never let a question change provider mid-loop: Sonnet and Haiku 5.5 thinking blocks only work in the account that produced them, and prompt caches are per provider.
  - Pass `only: [provider]` on every round instead of letting the Gateway fail over by itself.
- **Covered models stay off the table.** Keep Fable and Mythos models out of `models`. Anthropic retains conversation content by default only for those, and the privacy promise is that prompts aren't kept.
- **Other providers' models.** Through the same Anthropic-format endpoint they are possible, and the Gateway advertises them, but Ask depends on Claude's tool use, thinking and citations. Treat any non-Claude row as an experiment behind an eval (see Testing) before enabling it.
- **Zero data retention.** Turn on the Gateway's ZDR option per request. Confirm the exact `providerOptions` field name against the Gateway docs when building.

### GET /v1/me

Returns `{ user: { id, email? }, plan: "free" | "pro", renewsAt?, source?: "app_store" | "stripe",
ask: { questionsLeft, resetsAt, deepAvailable }, sermons: { hoursLeft, trialAvailable } }`. The app
calls it on launch (when signed in), after a purchase and when Ask opens, and shows the allowance
in Settings → Account.

### Errors

Every JSON error is `{ "error": { "type": string, "message": string, "upgrade"?: boolean, "retryAfter"?: number } }`.

| Status | type | Client behaviour |
|---|---|---|
| 400 | `invalid_request` | Bug: log it, show "Something went wrong" |
| 401 | `unauthenticated` | Refresh the token once, then show sign-in |
| 402 | `allowance_exhausted` | Show the Pro sheet (or the reset date, if already Pro) |
| 403 | `plan_required` | Deep study or cloud transcripts on Free: show the Pro sheet |
| 409 | `contract_unsupported` | "Update Linger to keep using Ask" |
| 413 | `too_large` | Conversation too long: suggest a new chat |
| 429 | `rate_limited` | Wait `retryAfter` seconds |
| 502 / 503 | `upstream_unavailable` | "Ask is unavailable right now" |

## State in Convex

The proxy is Convex's only client in v1. The app never talks to Convex directly; that comes later,
with sync.

- **Server calls.** Proxy-to-Convex calls use `ConvexHttpClient` and public functions wrapped in a
  `serverMutation` / `serverQuery` helper (convex-helpers `customMutation`). The helper requires an
  argument equal to the `LINGER_SERVER_SECRET` environment variable, compared in constant time.
  Nothing else is exposed.
- **Transactions.** Convex mutations are serializable transactions, so `beginRound` can check and
  increment allowances without races. A mutation's own code must finish within 1 s, which these
  small reads and writes do easily.
- **Rate limits.** Use the `@convex-dev/rate-limiter` component (token bucket per user: 20 rounds a
  minute, 10 questions a minute). Add a Vercel Firewall rate-limit rule per IP in front.

Tables (`convex/schema.ts`):

| Table | Fields | Indexes |
|---|---|---|
| `users` | `clerkId`, `createdAt`, `appAccountToken` (UUID for StoreKit), `stripeCustomerId?`, `trialHourUsed` | `by_clerk`, `by_app_account_token`, `by_stripe_customer` |
| `plans` | `key` (`free`, `pro`), `questionsPerMonth`, `questionsPerDay`, `deepCost`, `maxRounds`, `costCeilingMicros`, `sermonSecondsPerMonth`, `modelKey`, `deepModelKey?` | `by_key` |
| `models` | `key`, `gatewayModel`, `thinking`, `effort`, `maxTokens`, `enabled` | `by_key` |
| `routing` | singleton: `providers: string[]`, `note` | none |
| `entitlements` | `userId`, `source` (`app_store`, `stripe`, `grant`), `productId`, `status`, `periodStart`, `expiresAt`, `originalId`, `environment` | `by_user`, `by_original` |
| `usage` | `userId`, `period` (`2026-11`), `questions`, `deepQuestions`, `costMicros`, `sermonSeconds`, `day`, `questionsToday` | `by_user_period` |
| `questions` | `questionId`, `userId`, `model`, `provider`, `rounds`, `startedAt` | `by_question` |
| `reservations` | `userId`, `kind` (`ask`, `sermon`), `amountMicros` or `seconds`, `createdAt` | `by_created` |
| `ledger` | `userId`, `questionId?`, `kind`, `model?`, `provider?`, token counts, `audioSeconds?`, `costMicros`, `ttftMs?`, `durationMs`, `status`, `at` | `by_user_at` |
| `sermons` | `userId`, `blobPath`, `durationSec`, `assemblyId?`, `status`, `createdAt`, `error?` | `by_user`, `by_assembly` |
| `webhookEvents` | `source`, `eventId`, `receivedAt` | `by_source_event` (idempotency) |

The effective plan is Pro when any `entitlements` row is active (`status` active, or in grace, and
`expiresAt` in the future). Otherwise it is Free. `grant` rows cover comps, press and family.

Crons (`convex/crons.ts`):
- Every 5 minutes, sweep reservations older than 15 minutes and charge them (crash recovery).
- Nightly:
  - Reconcile App Store subscriptions through Apple's Get Notification History.
  - Delete `questions` older than 7 days.
  - Delete ledger rows older than 13 months.
- Hourly, fail `sermons` stuck in `transcribing` for over 3 hours and delete their blobs.

The ledger holds metadata only: no prompts, answers, transcripts or audio. That matches the
privacy story, and it is all billing and abuse review need.

## Sermon transcription through the proxy

The client already joins chunks and runs an AssemblyAI job. The proxy takes over the key, the
allowance and the upload:

1. **POST /v1/sermons** `{ recordingId, durationSec, bytes, contentType }`:
   - Check sign-in and the plan: Pro with hours left, or a Free account's unused trial hour.
   - Reserve `durationSec` against the allowance. Reject durations over 3 hours or sizes inconsistent with 16 kHz mono 16-bit WAV.
   - Create a `sermons` row and mint a **signed Vercel Blob PUT URL** for `sermons/<userId>/<random>.wav`. Private Blob and signed URLs are GA. The URL is valid for 30 minutes, with a content-type and size constraint.
   - Return `{ sermonId, uploadUrl }`.
2. The client PUTs the joined WAV straight to Blob. It never passes through a function, so the
   4.5 MB limit doesn't apply. About 115 MB an hour is fine on most connections. A later
   optimization is pure-Rust FLAC encoding (`flacenc`) to roughly halve it.
3. **POST /v1/sermons/:id/transcribe**:
   - Confirm the blob exists and that its size matches the duration.
   - Mint a **signed GET URL** (2 hours) and create the AssemblyAI job with the same settings `transcribe.rs` uses today (`universal-3-5-pro`, speaker labels, book names as `keyterms_prompt`).
   - Pass `webhook_url` → `/v1/webhooks/assemblyai`, with a per-job secret header.
4. **The webhook:**
   - Verify the secret, fetch the finished transcript and settle the actual `audio_duration` in Convex.
   - Delete the AssemblyAI job (which deletes AssemblyAI's copy) and the blob.
   - Keep the transcript JSON for 24 hours in Convex file storage for the client to fetch, then delete it.
5. **GET /v1/sermons/:id** returns `status` (`uploading`, `transcribing`, `done`, `error`) and, once
   done, the transcript in the same shape `transcribe::segments` builds today. The client saves it as
   `transcript.json`, as now. The desktop app polls every 5 s while a sermon is in progress. iOS
   can do the same, or receive a silent push later.

Free users without cloud hours get on-device transcription:
- Apple's SpeechTranscriber on macOS 26 and iOS 26, through a small Swift static library with a C ABI for the egui app.
- On Linux, `whisper-rs` with a small model.

On-device transcripts are lower quality, and the panel says so. The cloud transcript is the upgrade.

## Payments and entitlements

One user can be Pro from either source, and the plan is the same everywhere.

### iOS: StoreKit 2

- **Store setup.** One subscription group, "Linger Pro", with `pro.monthly` ($7.99) and
  `pro.yearly` ($69.99). Enroll in the Small Business Program for 15% commission.
- **Purchase.** The app purchases with `appAccountToken = users.appAccountToken`, a UUID minted by
  the server at user creation and returned by `/v1/me`. It then posts `Transaction.jwsRepresentation`
  to `POST /v1/billing/apple/transactions`. The server verifies it with Apple's official
  `@apple/app-store-server-library` (`SignedDataVerifier` with Apple's root certificates) and
  upserts the entitlement.
- **Lifecycle.** **App Store Server Notifications V2** arrive at `/v1/webhooks/apple`. They are
  verified the same way, made idempotent by `notificationUUID`, and keep renewals, grace periods,
  refunds and revocations current. The nightly cron replays anything missed.
- **Restore.** Restore Purchases re-posts current entitlements from `Transaction.currentEntitlements`.
- **Web link (US only).** A "Subscribe on the web" link to Stripe Checkout is optional. US link-out
  currently pays Apple nothing, but a 5% rate is proposed, so don't build the economics on it.

### Desktop (Developer ID) and Linux: Stripe

- **Checkout.** `POST /v1/billing/stripe/checkout` creates a Checkout Session for the same two
  prices and returns its URL. The Stripe customer is looked up or created by `clerkId`, and
  `client_reference_id` is the Convex user id. The app opens it in the browser.
  - Prefer **Stripe Managed Payments** (merchant of record, VAT and sales tax for about 3.5% more).
  - If it isn't available to the account, use Stripe Tax, or Paddle as the merchant-of-record fallback.
- **Return.** Success and cancel URLs land on `https://linger.app/billing/done`, which deep-links
  back to `linger://billing/done`. The app then re-fetches `/v1/me`.
- **Lifecycle.** `/v1/webhooks/stripe`, with its signature verified on the raw body, handles:
  - `checkout.session.completed`
  - `customer.subscription.created`, `customer.subscription.updated` and `customer.subscription.deleted`
  - `invoice.payment_failed`
- **Manage.** `POST /v1/billing/stripe/portal` returns a Customer Portal URL.

Clerk Billing was considered. It would put plans next to users, but it is a second subscription
system that iOS can't use, and Apple's purchases have to live in the same table anyway.
RevenueCat was considered too. It isn't needed with one product group, two sources and a backend
that exists regardless.

## Privacy and consent

- **Consent sheet.** The first Ask use shows a one-time sheet (guideline 5.1.2(i)):
  - Questions and the passages Ask reads are sent to Anthropic's Claude through Vercel.
  - Nothing is kept for training.
  - Conversations stay on the device.
- **Sermon consent.** The first cloud transcription names AssemblyAI. The user confirms they may
  record the speaker, and the sheet says the audio is deleted when the transcript is done.
- **Server logs.** Logs and traces carry the model, token counts, timings, status and a hashed user
  id, never bodies. Redact `Authorization`, and turn off request-body logging in Vercel.
- **Settings → Account.** It shows the plan, the remaining allowance, Manage subscription, Sign out
  and Delete account.

## Observability

- **Vercel Observability:** function latency and errors, plus the AI Gateway dashboard (spend,
  tokens and errors per model), which comes free with the Gateway.
- **Effect tracing:** `effect/observability`'s OTLP exporter sends spans to Grafana Cloud's free
  tier, or to Axiom through the Vercel Marketplace. The spans are `ask.begin`, `gateway.stream`
  (TTFT and duration), `ask.settle`, `sermon.*` and `webhook.*`.
- **Errors:** Sentry for unhandled defects (`Effect.catchAllDefect` at the edge).
- **Business:** the Convex dashboard over `ledger` and `usage` (questions per day, cost per user,
  ceiling hits). Set a daily email alert when total model spend passes a threshold.
- **Gateway spend:** a budget alert in Vercel as a backstop. The proxy's allowances remain the real
  control.

## Testing

**Effect unit tests** use `@effect/vitest` with test layers for `Store`, `Gateway`, `Auth` and the
other services:
- Validation rejects:
  - An unknown tool name.
  - A contract version older than the two supported.
  - A body over 1 MB.
  - `round` ≥ `maxRounds`.
- **Pinning.** A question started on Anthropic stays on Anthropic after the routing override flips
  to Bedrock, while a *new* question goes to Bedrock.
- **Allowances.**
  - The 30th Free question passes and the 31st is a 402.
  - A Deep study question counts 2.
  - Past the cost ceiling, round 0 downgrades to Haiku.
- **Settlement.** A stream aborted after `message_start` charges the reservation. A completed
  stream charges the actual usage (asymmetric token counts, so input and output prices can't be
  swapped unnoticed).

**Convex function tests** use `convex-test`:
- Concurrent `beginRound` calls at the allowance boundary never over-admit.
- Entitlement upserts are idempotent per `originalId`.
- Webhook event dedupe.

**Contract drift:**
- The `linger-core` test fails if `ask-v1.json` is stale.
- A server test fails if the JSON doesn't parse into the expected shape.

**Load:** 1,000 concurrent `/v1/ask` streams against a mock Gateway that trickles events for 60 s.
Check memory, the instance count and that every reservation settles.

**Payments:**
- Run Apple's sandbox and StoreKit Testing in Xcode for purchase, renewal, refund and revoke.
- Replay Stripe's test-mode events (`stripe trigger`).

**Model evaluation**, before changing any model row:
- Run a fixed set of about 50 real Ask questions on public-domain text through the candidate model.
- Score:
  - Citation validity (every `{{Book C:V}}` was read with a tool).
  - Tool-use correctness.
  - Answer length.
- Run it from `cargo xtask eval-ask` against the proxy's staging deployment.

## Environments and configuration

| | Production | Preview / staging |
|---|---|---|
| Vercel | `api.linger.app`, production branch `main` | every PR's preview URL |
| Clerk | Linger production instance | Linger development instance |
| Convex | production deployment | dev deployment per developer, one staging |
| Stripe | live | test |
| App Store | production | sandbox and Xcode StoreKit config |
| AssemblyAI | prod key | same key, its own project if possible |

Environment variables on Vercel. The Gateway needs none, because it uses OIDC.
- **Clerk:** `CLERK_SECRET_KEY`, `CLERK_JWT_ISSUER`, `CLERK_AUTHORIZED_PARTIES`.
- **Convex:** `CONVEX_URL`, `LINGER_SERVER_SECRET`.
- **AssemblyAI:** `ASSEMBLYAI_API_KEY`.
- **Blob:** `BLOB_READ_WRITE_TOKEN`, or OIDC if supported.
- **Apple:**
  - `APPLE_BUNDLE_ID`, `APPLE_APP_APPLE_ID`, `APPLE_ISSUER_ID`, `APPLE_KEY_ID`, `APPLE_PRIVATE_KEY`.
  - `APPLE_ENVIRONMENT` (the App Store Server API key is only needed for history and reconcile).
- **Stripe:** `STRIPE_SECRET_KEY`, `STRIPE_WEBHOOK_SECRET`, `STRIPE_PRICE_MONTHLY`, `STRIPE_PRICE_YEARLY`.
- **Observability:** `SENTRY_DSN`, `OTEL_EXPORTER_OTLP_ENDPOINT`.

The Convex deployment holds `LINGER_SERVER_SECRET`.

The app ships with only public values: the proxy URL, Clerk's publishable key and Frontend API URL,
and the OAuth client id.

## Build plan

### Phase 0: confirm and prepare (no server code)

1. Send the three licensing emails (API.Bible, Crossway, Tyndale).
2. Create the Clerk "Linger" application, with Apple, Google and email. Check which features
   need Pro.
3. Create the Vercel project with `server/` as its root, and link the Convex project.
   - Turn on AI Gateway for the team and add credits.
   - Request Anthropic and Bedrock capacity through the Gateway if needed.
4. Add `cargo xtask contract` and the drift test. Move the model settings out of `assistant.rs`
   into the contract.
5. **Public-domain gate:** Ask's tools read from the BSB (bundled) whatever the reader shows, and
   the panel names the translation. Ship this first, even before the proxy.
6. Remove the one-click ESV, NLT and API.Bible key flows from release builds until the emails are
   answered.

*Done when:* the emails are sent, the accounts exist, the contract generates, and Ask can't send
copyrighted text.

### Phase 1: Ask through the proxy, Free tier

1. Scaffold `server/` with Effect 4 pinned. Add the `AppConfig`, `Auth`, `Store` and `Gateway`
   services, the Convex schema with `plans` and `models` seeded, and the `serverMutation` helper.
2. Build `POST /v1/ask` with validation, `beginRound`, injection, pass-through streaming,
   metering and `settleRound`. Add the reservation sweeper cron and the rate limiter. Add
   `GET /v1/me`.
3. **Client:**
   - `assistant::Config` targets the proxy with a bearer token.
   - Add the slim body, `questionId` and `round`, and 402 / 429 / 409 handling in the Ask panel.
   - Keep the env-var direct mode for development only.
4. **Temporary sign-in:** to unblock phase 1 before phase 2's native flows, the desktop app accepts a
   Clerk token pasted in from a dev-only Settings field.

*Done when:* a release build answers Ask with no key in it, Free limits hold under the concurrency
test, and the load test passes.

### Phase 2: accounts

1. Desktop: OAuth PKCE through `ASWebAuthenticationSession` on macOS and loopback on Linux.
   Keychain or keyring storage, refresh and sign-out (revoking the refresh token).
2. iOS: the Clerk iOS SDK with native Sign in with Apple, and the session token on requests.
3. Settings → Account with the plan, allowance, sign-out and **Delete account** (`DELETE /v1/account`
   plus the Clerk webhook purge).
4. The Ask consent sheet.

*Done when:* sign-in works on macOS, iOS and Linux, and deletion meets the App Review requirement.

### Phase 3: Pro

1. Seed the `pro` plan, and add the `standard` and `deep` model rows. Add the Deep study toggle in
   the Ask panel (Pro only).
2. StoreKit 2 products, purchase with `appAccountToken`, `/v1/billing/apple/transactions`,
   ASSN V2 webhook, restore, and the nightly reconcile.
3. Stripe Checkout and Portal, the webhook, and the browser round-trip on desktop.
4. Pro sheet copy that states the allowances in questions and sermon-hours (3.1.2(c)).

*Done when:* one Pro subscription bought on either platform unlocks Pro on all three, and renewal,
refund and cancellation are tested in sandbox and test mode.

### Phase 4: sermons through the proxy

1. The `/v1/sermons` endpoints, signed Blob URLs, AssemblyAI with a webhook, settlement and
   cleanup.
2. Client: `transcribe.rs` gains a proxy backend alongside the env-key one. It uploads to the signed
   URL, then polls `/v1/sermons/:id`. The Free one-hour trial.
3. On-device transcription for Free: a SpeechTranscriber Swift shim on Apple platforms and
   `whisper-rs` on Linux.

*Done when:* a Pro user records a sermon on any platform and gets a cloud transcript, the allowance
moves by the audio's real duration, and nothing is left in Blob or AssemblyAI afterwards.

### Phase 5: hardening

1. The Bedrock routing override drill: flip `routing`, confirm that new questions move and
   in-flight ones don't.
2. The model eval harness (`cargo xtask eval-ask`), and a first eval of Sonnet 5.5 against Opus 5.5
   for `standard`.
3. Spend alerts, the Sentry and OTLP dashboards, and a runbook for "Claude down", "Gateway down"
   and "Convex down" (Ask shows unavailable, and reading is unaffected).
4. A review of the first month's usage, then retune `plans` from the data.

## Later

- **Church plans.** A church records each sermon once and pays a flat monthly fee. Members follow
  the church and get its transcripts in Ask. This is cheaper per listener, and the church holds the
  rights to its own sermons. It needs organizations, which Clerk has, and a shared-transcript
  table.
- **Sync.** Sync Recents, notes and highlights as a Pro feature. The Convex Swift and Rust clients
  would talk to Convex directly, with Clerk JWTs (`auth.config.ts`), and Convex would enforce
  per-user access in its functions.
- **Bring your own Anthropic key.** A power-user setting that skips the proxy for Ask; it can't
  apply to cloud transcription.
- **Licensed translations in Ask.** Only with written consent from API.Bible and the rights holder.
- **A Mac App Store build.** It would get native Sign in with Apple and IAP on the Mac, at the cost
  of sandboxing ScreenCaptureKit system-audio capture.

## Open questions

| Question | Who | Blocks |
|---|---|---|
| Can a paid Linger keep users' own API.Bible keys, and the ESV and NLT integrations? | API.Bible, Crossway, Tyndale | Phase 3 release |
| Do Clerk OAuth applications, passkeys and loopback redirects work on Hobby for a second application? | Clerk dashboard | Phase 2 |
| Does Clerk revoke Sign in with Apple tokens on user deletion? | Clerk docs or support | Phase 2 |
| What are the exact Gateway field names for ZDR and provider pinning on `/v1/messages`? | Vercel docs | Phase 1 |
| Is Stripe Managed Payments available to Fabrika LLC? | Stripe | Phase 3 |
| How accurate is AssemblyAI on reverberant church audio? Run a bake-off on 3–5 real sermons against ElevenLabs Scribe v2 | Us | Phase 4 (already in use, so this is a quality check) |
| Will Free's 30 Haiku questions convert, or does Free need a few Sonnet questions to show the difference? | First month's data | Phase 5 retune |
