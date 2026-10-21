/**
 * A zip file nothing but a zip reader is needed for (docs/plans/m2-search-and-ask.md, "Export"):
 * every entry stored, uncompressed, with UTF-8 names, which every unzipper opens. Audio and
 * photos don't compress anyway, and text is a small part of a library. Pure: no `ctx`.
 */

export interface Entry {
  readonly name: string
  readonly bytes: Uint8Array
}

const TABLE = (() => {
  const t = new Uint32Array(256)
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    t[n] = c >>> 0
  }
  return t
})()

export const crc32 = (bytes: Uint8Array): number => {
  let c = 0xffffffff
  for (const b of bytes) c = TABLE[(c ^ b) & 0xff]! ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}

/** MS-DOS date and time, as zip entries carry them. */
const dosTime = (ms: number) => {
  const d = new Date(ms)
  const time = (d.getUTCHours() << 11) | (d.getUTCMinutes() << 5) | (d.getUTCSeconds() >> 1)
  const date = ((Math.max(1980, d.getUTCFullYear()) - 1980) << 9) | ((d.getUTCMonth() + 1) << 5) | d.getUTCDate()
  return { time, date }
}

/** The zip holding `entries`, dated `at`. */
export const zip = (entries: ReadonlyArray<Entry>, at: number): Uint8Array<ArrayBuffer> => {
  const encoder = new TextEncoder()
  const { time, date } = dosTime(at)
  const locals: Array<Uint8Array> = []
  const centrals: Array<Uint8Array> = []
  let offset = 0
  for (const entry of entries) {
    const name = encoder.encode(entry.name)
    const crc = crc32(entry.bytes)
    const local = new DataView(new ArrayBuffer(30 + name.length))
    local.setUint32(0, 0x04034b50, true)
    local.setUint16(4, 20, true) // version needed: 2.0
    local.setUint16(6, 0x0800, true) // flags: UTF-8 names
    local.setUint16(8, 0, true) // method: stored
    local.setUint16(10, time, true)
    local.setUint16(12, date, true)
    local.setUint32(14, crc, true)
    local.setUint32(18, entry.bytes.length, true)
    local.setUint32(22, entry.bytes.length, true)
    local.setUint16(26, name.length, true)
    local.setUint16(28, 0, true)
    new Uint8Array(local.buffer).set(name, 30)
    const central = new DataView(new ArrayBuffer(46 + name.length))
    central.setUint32(0, 0x02014b50, true)
    central.setUint16(4, 20, true) // made by: 2.0, MS-DOS attributes
    central.setUint16(6, 20, true)
    central.setUint16(8, 0x0800, true)
    central.setUint16(10, 0, true)
    central.setUint16(12, time, true)
    central.setUint16(14, date, true)
    central.setUint32(16, crc, true)
    central.setUint32(20, entry.bytes.length, true)
    central.setUint32(24, entry.bytes.length, true)
    central.setUint16(28, name.length, true)
    central.setUint32(42, offset, true)
    new Uint8Array(central.buffer).set(name, 46)
    locals.push(new Uint8Array(local.buffer), entry.bytes)
    centrals.push(new Uint8Array(central.buffer))
    offset += local.byteLength + entry.bytes.length
  }
  const directory = centrals.reduce((n, c) => n + c.length, 0)
  const end = new DataView(new ArrayBuffer(22))
  end.setUint32(0, 0x06054b50, true)
  end.setUint16(8, entries.length, true)
  end.setUint16(10, entries.length, true)
  end.setUint32(12, directory, true)
  end.setUint32(16, offset, true)
  const parts = [...locals, ...centrals, new Uint8Array(end.buffer)]
  const out = new Uint8Array(new ArrayBuffer(parts.reduce((n, p) => n + p.length, 0)))
  let written = 0
  for (const p of parts) {
    out.set(p, written)
    written += p.length
  }
  return out
}
