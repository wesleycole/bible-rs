/**
 * Chunks (docs/plans/library.md, "The data model"): the unit of search and citation, rebuilt from
 * an item's text whenever it changes. A note is one chunk. A sermon's transcript becomes
 * paragraphs of about TARGET words: a paragraph closes at a speaker change or a pause once it has
 * MIN words, or at TARGET words anywhere, and never inside a sentence. One long turn of speech is
 * split by sentence, its times interpolated by position. Pure: no `ctx`.
 */
import type { Transcript } from "../../shared/records.js"

export interface Chunk {
  readonly ordinal: number
  readonly text: string
  readonly startMs?: number
  readonly endMs?: number
  readonly page?: number
  readonly focus: boolean
  readonly unreviewed: boolean
}

/** Words a paragraph aims for. */
export const TARGET = 200
/** Words a paragraph has before a speaker change or a pause may end it. */
const MIN = 50
/** A silence this long between turns is a pause. */
const PAUSE = 1500

const words = (text: string) => text.split(/\s+/).filter((w) => w.length > 0).length

interface Sentence {
  readonly text: string
  readonly startMs: number
  readonly endMs: number
  readonly speaker: string | undefined
}

/** A segment's sentences, each given the share of the segment's time its characters take. */
const sentencesOf = (s: Transcript["segments"][number]): Array<Sentence> => {
  const parts = s.text.split(/(?<=[.!?])\s+/).map((p) => p.trim()).filter((p) => p.length > 0)
  const total = parts.reduce((n, p) => n + p.length, 0)
  const span = s.endMs - s.startMs
  let at = 0
  return parts.map((text) => {
    const startMs = s.startMs + Math.round((span * at) / total)
    at += text.length
    return { text, startMs, endMs: s.startMs + Math.round((span * at) / total), speaker: s.speaker }
  })
}

/** A transcript's paragraphs, with the time each one spans. */
export const chunkTranscript = (transcript: Transcript, unreviewed: boolean): Array<Chunk> => {
  const chunks: Array<Chunk> = []
  let open: Array<Sentence> = []
  let count = 0
  const close = () => {
    if (open.length === 0) return
    chunks.push({
      ordinal: chunks.length,
      text: open.map((s) => s.text).join(" "),
      startMs: open[0]!.startMs,
      endMs: open.at(-1)!.endMs,
      focus: false,
      unreviewed
    })
    open = []
    count = 0
  }
  for (const segment of transcript.segments) {
    for (const sentence of sentencesOf(segment)) {
      const last = open.at(-1)
      const boundary = last !== undefined && (last.speaker !== sentence.speaker || sentence.startMs - last.endMs >= PAUSE)
      if (count >= TARGET || (boundary && count >= MIN)) close()
      open.push(sentence)
      count += words(sentence.text)
    }
  }
  close()
  return chunks
}

/** "[1:02:03]" or "[02:03]": how a time is shown in text, to a model and a person alike. */
export const stamp = (ms: number) => {
  const s = Math.floor(ms / 1000)
  const pad = (n: number) => String(n).padStart(2, "0")
  const h = Math.floor(s / 3600)
  return `[${h > 0 ? `${h}:` : ""}${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}]`
}

/** A note is one chunk. */
export const chunkNote = (text: string): Array<Chunk> =>
  text.trim().length === 0 ? [] : [{ ordinal: 0, text: text.trim(), focus: false, unreviewed: false }]
