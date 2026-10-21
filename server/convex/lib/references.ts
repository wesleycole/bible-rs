/**
 * References a model wrote ("Romans 8:28", "Rom 8:28-30", "Ps 23", "John 3:16-4:2") as verse
 * ranges on the canonical grid, from canon.json. Strict where it matters: a book that isn't a
 * book, or a verse past the chapter's end, is nothing rather than a guess. Pure: no `ctx`.
 */
import canon from "./canon.json" with { type: "json" }

export interface VerseRange {
  readonly from: number
  readonly to: number
}

/** Names the model may use that aren't the canon's own. */
const ALIASES: Record<string, string> = {
  psalm: "Psalms",
  "song of songs": "Song of Solomon",
  canticles: "Song of Solomon",
  revelations: "Revelation"
}

const normal = (s: string) =>
  s.toLowerCase().replace(/\./g, "").replace(/^(i{1,3})\s/, (_, n: string) => `${n.length} `).replace(/^(\d)(?=[a-z])/, "$1 ").replace(/\s+/g, " ").trim()

/** The book's index on the grid, by its name, an alias, or a prefix of three or more letters (the first in canon order). */
const bookOf = (name: string): number => {
  const n = normal(name)
  const wanted = (ALIASES[n] ?? n).toLowerCase()
  const exact = canon.books.findIndex((b) => b.name.toLowerCase() === wanted)
  if (exact >= 0) return exact
  const letters = wanted.replace(/^\d /, "")
  if (letters.length < 3) return -1
  return canon.books.findIndex((b) => b.name.toLowerCase().startsWith(wanted))
}

const id = (book: number, chapter: number, verse: number) => book * 1_000_000 + chapter * 1_000 + verse

const PATTERN = /^(.+?)\s+(\d+)(?::(\d+))?(?:\s*[-–—]\s*(?:(\d+):)?(\d+))?$/

/** The range a reference names, if it is on the grid: a chapter alone is the whole chapter. */
export const parseReference = (text: string): VerseRange | null => {
  const m = PATTERN.exec(text.trim())
  if (!m) return null
  const book = bookOf(m[1]!)
  if (book < 0) return null
  const chapters = canon.books[book]!.verses
  const chapter = Number(m[2])
  const verses = chapters[chapter - 1]
  if (verses === undefined) return null
  if (m[3] === undefined) {
    // "Romans 8", or "Romans 8-9" as chapters.
    const last = m[5] === undefined ? chapter : Number(m[5])
    const lastVerses = chapters[last - 1]
    if (m[4] !== undefined || lastVerses === undefined || last < chapter) return null
    return { from: id(book, chapter, 1), to: id(book, last, lastVerses) }
  }
  const verse = Number(m[3])
  if (verse < 1 || verse > verses) return null
  if (m[5] === undefined) return { from: id(book, chapter, verse), to: id(book, chapter, verse) }
  const toChapter = m[4] === undefined ? chapter : Number(m[4])
  const toVerses = chapters[toChapter - 1]
  const toVerse = Number(m[5])
  if (toVerses === undefined || toVerse < 1 || toVerse > toVerses) return null
  const range = { from: id(book, chapter, verse), to: id(book, toChapter, toVerse) }
  return range.from <= range.to ? range : null
}
