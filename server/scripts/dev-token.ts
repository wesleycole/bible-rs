/**
 * Local sign-in without Clerk: mints RS256 JWTs shaped like Clerk's session tokens (or, with
 * --oauth, its OAuth access tokens), signed by a key pair kept in .data/dev-clerk. Point the proxy
 * at the public key with CLERK_JWT_KEY (printed by --env) and CLERK_JWT_ISSUER=https://clerk.linger.test.
 *
 *   pnpm dev:token [user_id] [--oauth] [--minutes 60]
 *   pnpm dev:token --env        # prints CLERK_JWT_KEY, CLERK_JWT_ISSUER and CLERK_JWKS (a data URI)
 */
import { createPrivateKey, createPublicKey, generateKeyPairSync, sign } from "node:crypto"
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"

export const DEV_ISSUER = "https://clerk.linger.test"
const dir = join(import.meta.dirname, "..", ".data", "dev-clerk")

export const devKeys = () => {
  const privatePath = join(dir, "private.pem")
  if (!existsSync(privatePath)) {
    mkdirSync(dir, { recursive: true })
    const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 })
    writeFileSync(privatePath, privateKey.export({ type: "pkcs8", format: "pem" }), { mode: 0o600 })
  }
  const privateKey = createPrivateKey(readFileSync(privatePath))
  const publicPem = createPublicKey(privateKey).export({ type: "spki", format: "pem" }).toString()
  const jwk = createPublicKey(privateKey).export({ format: "jwk" })
  const jwks = { keys: [{ ...jwk, kid: "ins_dev", alg: "RS256", use: "sig" }] }
  return { privateKey, publicPem, jwksDataUri: `data:text/plain;charset=utf-8;base64,${Buffer.from(JSON.stringify(jwks)).toString("base64")}` }
}

const b64 = (v: unknown) => Buffer.from(JSON.stringify(v)).toString("base64url")

export const mintToken = (sub: string, opts: { oauth?: boolean; minutes?: number; email?: string } = {}) => {
  const { privateKey } = devKeys()
  const now = Math.floor(Date.now() / 1000)
  const header = { alg: "RS256", typ: opts.oauth ? "at+jwt" : "JWT", kid: "ins_dev" }
  const payload = {
    iss: DEV_ISSUER,
    sub,
    iat: now,
    nbf: now - 5,
    exp: now + 60 * (opts.minutes ?? 60),
    ...(opts.oauth
      ? { client_id: "linger-desktop", scope: "openid profile email offline_access", jti: crypto.randomUUID() }
      : { sid: `sess_${crypto.randomUUID().slice(0, 8)}`, azp: "linger://app", v: 2 }),
    ...(opts.email ? { email: opts.email } : {})
  }
  const input = `${b64(header)}.${b64(payload)}`
  return `${input}.${sign("sha256", Buffer.from(input), privateKey).toString("base64url")}`
}

if (import.meta.main) {
  const args = process.argv.slice(2)
  if (args.includes("--env")) {
    const { publicPem, jwksDataUri } = devKeys()
    const pem = publicPem.trim().replace(/\n/g, "\\n")
    console.log(`CLERK_JWT_KEY="${pem}"\nCLERK_JWT_ISSUER=${DEV_ISSUER}\nCLERK_JWKS=${jwksDataUri}`)
  } else {
    const minutes = args.includes("--minutes") ? Number(args[args.indexOf("--minutes") + 1]) : undefined
    const sub = args.find((a) => a.startsWith("user_")) ?? "user_dev"
    console.log(mintToken(sub, { oauth: args.includes("--oauth"), minutes }))
  }
}
