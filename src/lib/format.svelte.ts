import { t } from './i18n.svelte'
import type { Clock, DateFormat } from './types'

// Dates follow the system's region, not the UI's language, unless the settings pick a
// clock or an order. Month and weekday names follow the region either way.
let clock = $state<Clock>('system')
let dateFormat = $state<DateFormat>('system')

/** Formats dates as the `clock` and `dateFormat` settings say. */
export function setDateTime(newClock: Clock, newDateFormat: DateFormat) {
  clock = newClock
  dateFormat = newDateFormat
}

const hourCycles = { system: undefined, h24: 'h23', h12: 'h12' } as const
const systemHourCycle = new Intl.DateTimeFormat(undefined, { hour: 'numeric' }).resolvedOptions().hourCycle
const systemIs12Hour = systemHourCycle === 'h11' || systemHourCycle === 'h12'

/** Hour and minute on the clock: a 12-hour clock doesn't pad the hour. */
const hourMinute = $derived.by((): Intl.DateTimeFormatOptions => {
  const twelve = clock === 'h12' || (clock === 'system' && systemIs12Hour)
  return { hour: twelve ? 'numeric' : '2-digit', minute: '2-digit', hourCycle: hourCycles[clock] }
})
const time = $derived(new Intl.DateTimeFormat(undefined, hourMinute))
const full = $derived(
  new Intl.DateTimeFormat(undefined, { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric', ...hourMinute }),
)
const day = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' })
const dayInYear = new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
const longDay = new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'long', day: 'numeric' })
const weekday = new Intl.DateTimeFormat(undefined, { weekday: 'long' })

/** The date's parts, with the month as a name: short when the region has one, long
 * when only that is a name (Finnish, Czech). `undefined` when the region names months
 * by number (Japanese, Chinese). */
function namedParts(d: Date, long: boolean): Record<string, string> | undefined {
  for (const f of long ? [longDay] : [dayInYear, longDay]) {
    const parts = Object.fromEntries(f.formatToParts(d).map((p) => [p.type, p.value]))
    if (/\p{L}/u.test(parts.month ?? '')) return parts
  }
}

const pad = (n: number) => String(n).padStart(2, '0')

/** "12 Oct 2026", "Oct 12, 2026" or "2026-10-12", in `format`. ISO always has the year.
 * A region that names months by number gets its own format instead. */
function dateOf(d: Date, long: boolean, withYear: boolean, format = dateFormat): string {
  if (format === 'iso') return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
  const parts = format === 'system' ? undefined : namedParts(d, long)
  if (!parts) return (long ? longDay : withYear ? dayInYear : day).format(d)
  const dayMonth = format === 'dayMonth' ? `${parts.day} ${parts.month}` : `${parts.month} ${parts.day}`
  if (!withYear) return dayMonth
  return `${dayMonth}${format === 'dayMonth' ? ' ' : ', '}${parts.year}`
}

/** "12 Oct", with the year when it isn't this year's. */
function shortDate(d: Date): string {
  return dateOf(d, false, d.getFullYear() !== new Date().getFullYear())
}

/** Today's date in `format`, to show what it looks like. */
export function dateExample(format: DateFormat): string {
  return dateOf(new Date(), false, true, format)
}

/** Time for today, a short date otherwise. Reactive. */
export function formatTime(ms: number): string {
  const d = new Date(ms)
  return d.toDateString() === new Date().toDateString() ? time.format(d) : shortDate(d)
}

/** "Today at 20:34", "Yesterday at 09:12" or "12 Oct at 09:12". Reactive. */
export function formatSynced(ms: number): string {
  const d = new Date(ms)
  const yesterday = new Date()
  yesterday.setDate(yesterday.getDate() - 1)
  const at = time.format(d)
  if (d.toDateString() === new Date().toDateString()) return t('format.today', { time: at })
  if (d.toDateString() === yesterday.toDateString()) return t('format.yesterday', { time: at })
  return t('format.dayAt', { date: shortDate(d), time: at })
}

/** The weekday, date and time, such as "Monday 12 October 2026, 20:34". Reactive. */
export function formatFull(ms: number): string {
  const d = new Date(ms)
  if (dateFormat === 'system') return full.format(d)
  return `${weekday.format(d)}${dateFormat === 'monthDay' ? ',' : ''} ${dateOf(d, true, true)}, ${time.format(d)}`
}

/** A message as a sentence: capitalized, with a full stop. Rust's errors are written
 * lowercase, to follow other text. */
export function sentence(message: string): string {
  const s = message.trim()
  if (!s) return s
  return s[0].toUpperCase() + s.slice(1) + (/[.!?]$/.test(s) ? '' : '.')
}

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`)
}

/**
 * Completes what the user typed into a server address: `https://` when no scheme is
 * given. Signing in finds the API path itself, such as FreshRSS's `/api/greader.php`.
 */
export function serverUrl(input: string): string {
  let s = input.trim()
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(s)) s = `https://${s}`
  try {
    return new URL(s).toString().replace(/\/$/, '')
  } catch {
    return s
  }
}
