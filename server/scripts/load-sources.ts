/**
 * Loads outside sources (docs/plans/m7-outside-sources.md) into a Convex deployment: each
 * `data/sources/<work>.jsonl.zst` that `cargo xtask build-sources` wrote goes through the
 * server-secret functions in convex/sources.ts, in batches of entries under a version named by the
 * file's SHA-256, then the work's row is flipped to that version in one write, then the version it
 * replaced is purged in steps. Readers never see a half-loaded work, and an unchanged file is skipped.
 *
 *   CONVEX_URL=… LINGER_SERVER_SECRET=… npx tsx scripts/load-sources.ts ../data/sources/*.jsonl.zst
 */
import { createHash } from "node:crypto"
import { readFileSync } from "node:fs"
import { zstdDecompressSync } from "node:zlib"
import { ConvexHttpClient } from "convex/browser"
import { anyApi } from "convex/server"

/** Entries per `sources.load` call, and the most JSON one call carries. */
const BATCH = 500
const BATCH_BYTES = 4 * 1024 * 1024

const need = (k: string) => process.env[k] ?? (console.error(`${k} is unset`), process.exit(1))
const [url, secret] = [need("CONVEX_URL"), need("LINGER_SERVER_SECRET")]
const files = process.argv.slice(2)
if (files.length === 0) {
  console.error("usage: load-sources.ts <work>.jsonl.zst ...")
  process.exit(1)
}

const client = new ConvexHttpClient(url)
const active = new Map<string, string>(
  ((await client.query(anyApi.sources.works, { secret })) as Array<{ key: string; version: string }>).map((w) => [w.key, w.version])
)

for (const file of files) {
  const bytes = readFileSync(file)
  const version = createHash("sha256").update(bytes).digest("hex").slice(0, 16)
  const lines = zstdDecompressSync(bytes).toString("utf8").split("\n").filter((l) => l !== "")
  const { work } = JSON.parse(lines[0]!) as {
    work: { key: string; title: string; author: string; year: number; tradition: string; kind: string; licence: string }
  }
  const meta = { key: work.key, title: work.title, author: work.author, year: work.year, tradition: work.tradition, kind: work.kind, licence: work.licence }
  const previous = active.get(work.key)
  if (previous === version) {
    console.log(`${work.key}: ${version} is already loaded`)
    continue
  }
  const entries = lines.slice(1).map((l) => JSON.parse(l) as { cites?: Array<[number, number]> })
  let loaded = 0
  for (let at = 0; at < entries.length;) {
    const batch: Array<unknown> = []
    let size = 0
    while (at < entries.length && batch.length < BATCH && (batch.length === 0 || size < BATCH_BYTES)) {
      const e = entries[at++]!
      batch.push(e.cites === undefined ? e : { ...e, cites: e.cites.map(([from, to]) => ({ from, to })) })
      size += JSON.stringify(e).length
    }
    await client.mutation(anyApi.sources.load, { secret, work: work.key, version, kind: work.kind, entries: batch })
    loaded += batch.length
    process.stdout.write(`\r${work.key}: ${loaded}/${entries.length} entries`)
  }
  await client.mutation(anyApi.sources.commit, { secret, ...meta, version })
  console.log(`\r${work.key}: ${loaded} entries committed as ${version}${previous ? `, replacing ${previous}` : ""}`)
  if (previous !== undefined) {
    while ((await client.mutation(anyApi.sources.purge, { secret, work: work.key, version: previous })).more) {
      // Another step.
    }
    console.log(`${work.key}: ${previous} purged`)
  }
}
