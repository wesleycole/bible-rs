/**
 * Real Clerk tokens for the test user on the development instance, without a browser: signs
 * `amp+clerk_test@example.com` in through Clerk's Frontend API with test mode's email code
 * (424242), as the phone's hosted sign-in would, then runs the desktop's OAuth flow (PKCE, the
 * "Linger Desktop" client) on that session. The session is kept in .data/clerk-test.json, so later
 * calls mint fresh tokens from it.
 *
 *   pnpm clerk-test-token              # signs in; prints { sub, sessionToken, accessToken, refreshToken }
 *   pnpm clerk-test-token --session    # prints a fresh session token (they last 60 s)
 *   pnpm clerk-test-token --oauth      # prints a fresh OAuth access token (a day)
 */
import { createHash, randomBytes } from "node:crypto"
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"

// The same overrides and defaults as crates/core/src/auth.rs (OAuthConfig::from_env).
const FAPI = (process.env.LINGER_CLERK_URL?.trim() || "https://elegant-mink-67.clerk.accounts.dev").replace(/\/+$/, "")
const CLIENT_ID = process.env.LINGER_OAUTH_CLIENT_ID?.trim() || "XDg0tXKW3CgO9rJy"
const EMAIL = "amp+clerk_test@example.com"
const CODE = "424242"
const REDIRECT = "http://127.0.0.1:53682/callback"
const HEADERS = { "content-type": "application/x-www-form-urlencoded", origin: "http://localhost:3000", "user-agent": "linger-dev" }
const statePath = join(import.meta.dirname, "..", ".data", "clerk-test.json")

interface Saved {
  readonly devBrowser: string
  readonly sessionId: string
}

const fapi = async (path: string, db: string | null, body?: Record<string, string>): Promise<any> => {
  const url = new URL(path, FAPI)
  url.searchParams.set("_clerk_js_version", "5")
  if (db) url.searchParams.set("__clerk_db_jwt", db)
  const res = await fetch(url, { method: "POST", headers: HEADERS, body: new URLSearchParams(body ?? {}) })
  const json = await res.json()
  if (!res.ok) throw new Error(`${path}: HTTP ${res.status} ${JSON.stringify(json).slice(0, 300)}`)
  return json
}

const signIn = async (): Promise<Saved> => {
  const db = (await fapi("/v1/dev_browser", null)).token as string
  let sessionId: string
  try {
    const signIn = (await fapi("/v1/client/sign_ins", db, { identifier: EMAIL })).response
    const factor = signIn.supported_first_factors.find((f: any) => f.strategy === "email_code")
    await fapi(`/v1/client/sign_ins/${signIn.id}/prepare_first_factor`, db, { strategy: "email_code", email_address_id: factor.email_address_id })
    sessionId = (await fapi(`/v1/client/sign_ins/${signIn.id}/attempt_first_factor`, db, { strategy: "email_code", code: CODE })).response.created_session_id
  } catch {
    // The test user doesn't exist on this instance yet: sign it up with the same code.
    const signUp = (await fapi("/v1/client/sign_ups", db, { email_address: EMAIL })).response
    await fapi(`/v1/client/sign_ups/${signUp.id}/prepare_verification`, db, { strategy: "email_code" })
    sessionId = (await fapi(`/v1/client/sign_ups/${signUp.id}/attempt_verification`, db, { strategy: "email_code", code: CODE })).response.created_session_id
  }
  const saved = { devBrowser: db, sessionId }
  mkdirSync(join(statePath, ".."), { recursive: true })
  writeFileSync(statePath, JSON.stringify(saved), { mode: 0o600 })
  return saved
}

const sessionToken = async (s: Saved): Promise<string> => (await fapi(`/v1/client/sessions/${s.sessionId}/tokens`, s.devBrowser)).jwt

const oauthTokens = async (s: Saved) => {
  const verifier = randomBytes(32).toString("base64url")
  const challenge = createHash("sha256").update(verifier).digest("base64url")
  const authorize = new URL("/oauth/authorize", FAPI)
  for (const [k, v] of Object.entries({
    response_type: "code",
    client_id: CLIENT_ID,
    redirect_uri: REDIRECT,
    scope: "openid profile email offline_access",
    state: "linger-dev",
    code_challenge: challenge,
    code_challenge_method: "S256",
    __clerk_db_jwt: s.devBrowser
  })) authorize.searchParams.set(k, v)
  let url = authorize.toString()
  for (let hop = 0; hop < 5 && !url.startsWith(REDIRECT); hop++) {
    const res = await fetch(url, { redirect: "manual", headers: { "user-agent": HEADERS["user-agent"] } })
    const location = res.headers.get("location")
    if (!location) throw new Error(`/oauth/authorize answered ${res.status} without a redirect`)
    url = new URL(location, url).toString()
  }
  const code = new URL(url).searchParams.get("code")
  if (!code) throw new Error(`no code in ${url}`)
  const res = await fetch(new URL("/oauth/token", FAPI), {
    method: "POST",
    headers: HEADERS,
    body: new URLSearchParams({ grant_type: "authorization_code", code, redirect_uri: REDIRECT, client_id: CLIENT_ID, code_verifier: verifier })
  })
  if (!res.ok) throw new Error(`/oauth/token: HTTP ${res.status} ${await res.text()}`)
  return (await res.json()) as { access_token: string; refresh_token: string; expires_in: number }
}

const subOf = (jwt: string): string => JSON.parse(Buffer.from(jwt.split(".")[1]!, "base64url").toString()).sub

if (import.meta.main) {
  const args = process.argv.slice(2)
  const saved: Saved = existsSync(statePath) && args.length > 0 ? JSON.parse(readFileSync(statePath, "utf8")) : await signIn()
  if (args.includes("--session")) {
    console.log(await sessionToken(saved))
  } else if (args.includes("--oauth")) {
    console.log((await oauthTokens(saved)).access_token)
  } else {
    const session = await sessionToken(saved)
    const oauth = await oauthTokens(saved)
    console.log(JSON.stringify({ sub: subOf(session), sessionToken: session, accessToken: oauth.access_token, refreshToken: oauth.refresh_token }, null, 2))
  }
}
