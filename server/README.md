# Linger server

The backend from [docs/plans/backend.md](../docs/plans/backend.md): an Effect 4 proxy deployed as one
Vercel Function (`api/index.ts`), with Convex for accounts, allowances and the ledger.

## Run it locally

```sh
pnpm install
pnpm dev:convex          # anonymous local Convex on :3210; pushes convex/ on save
npx convex run admin:seed  # once: plans, models and routing
npx convex env set LINGER_SERVER_SECRET <same value as .env.local>
npx convex env set LINGER_PROXY_URL http://127.0.0.1:3000
pnpm dev                 # the proxy on :3000
```

`.env.local` (gitignored) needs `CONVEX_URL`, `LINGER_SERVER_SECRET`, `CLERK_JWT_KEY`,
`CLERK_JWT_ISSUER`, `BLOB_DRIVER=local` and `PUBLIC_URL`, plus `AI_GATEWAY_API_KEY` and
`ASSEMBLYAI_API_KEY` (or `ASSEMBLY_AI_API_KEY`) from the environment. Without Clerk, sign requests
with a local dev key:

```sh
npx tsx scripts/dev-token.ts --env        # CLERK_JWT_KEY and CLERK_JWT_ISSUER for .env.local
npx tsx scripts/dev-token.ts user_ruth    # a session token for that user
```

Locally, sermon audio is stored under `.data/blob` behind URLs the proxy signs and serves
(`/v1/dev/blob/*`). AssemblyAI can't call a loopback URL back, so jobs start without a webhook and
`GET /v1/sermons/:id` finishes them when they're done.

`scripts/convex-local.sh` runs Convex's local backend under Nix's glibc on hosts whose glibc is
older than the backend needs (Debian 12).

## Check it

```sh
pnpm typecheck   # the proxy, tests and scripts, then convex/
pnpm test        # vitest: real routes and real Convex functions (convex-test), vendor fakes
```

Load test: 1,000 concurrent Ask streams against a mock Gateway that trickles each answer over 60 s.

```sh
PORT=4100 pnpm mock-gateway
PORT=3100 AI_GATEWAY_URL=http://127.0.0.1:4100 npx tsx --env-file=.env.local src/dev/serve.ts
pnpm load-test --url http://127.0.0.1:3100 --n 1000 --pid <proxy pid>
npx convex run admin:stats '{"sinceMs": <since from the report>}'   # every reservation settled
```

## Deploy

Production today is the Vercel project `linger-api` (https://linger-api.vercel.app), Convex
deployment `expert-rabbit-665`, and Clerk's development instance; see "Environments and
configuration" in the plan.

- **Proxy:** `vercel deploy --prod` from `server/` (the project isn't connected to Git yet).
  `vercel.json` turns on Fluid compute in `iad1`, allows 800 s streams and rewrites `/v1/*` to
  `api/index.ts`. The variables are in `src/services/AppConfig.ts`; the Gateway uses the
  deployment's OIDC token, so `AI_GATEWAY_API_KEY` isn't set there. Blob is the private store
  `linger-sermons`, connected to the project.
- **Convex:** `CONVEX_DEPLOY_KEY=<production deploy key> npx convex deploy`, then
  `npx convex run admin:seed --prod` after changing plans or models. Its crons call back into the
  proxy at `LINGER_PROXY_URL` with `LINGER_SERVER_SECRET`, which must match the proxy's.
- **Webhooks:** Clerk sends `user.deleted` to `/v1/webhooks/clerk`; Stripe and Apple point at
  `/v1/webhooks/stripe` and `/v1/webhooks/apple` once they're configured.
