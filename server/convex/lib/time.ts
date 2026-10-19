const pad = (n: number) => String(n).padStart(2, "0")

/** "2026-11", in UTC: the month an allowance counts. */
export const periodOf = (ms: number): string => {
  const d = new Date(ms)
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}`
}

/** "2026-11-04", in UTC: the day the daily cap counts. */
export const dayOf = (ms: number): string => {
  const d = new Date(ms)
  return `${periodOf(ms)}-${pad(d.getUTCDate())}`
}

/** The first instant of the next UTC month, when monthly allowances reset. */
export const nextPeriodStart = (ms: number): number => {
  const d = new Date(ms)
  return Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1)
}

/** The next UTC midnight, when the daily cap resets. */
export const nextDayStart = (ms: number): number => {
  const d = new Date(ms)
  return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate() + 1)
}

export const MINUTE = 60_000
export const HOUR = 60 * MINUTE
export const DAY = 24 * HOUR
