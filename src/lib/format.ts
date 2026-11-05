const time = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' })
const day = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' })
const dayInYear = new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
const full = new Intl.DateTimeFormat(undefined, { dateStyle: 'full', timeStyle: 'short' })

/** "12 Oct", with the year when it isn't this year's. */
function shortDate(d: Date): string {
  return (d.getFullYear() === new Date().getFullYear() ? day : dayInYear).format(d)
}

/** Time for today, a short date otherwise. */
export function formatTime(ms: number): string {
  const d = new Date(ms)
  return d.toDateString() === new Date().toDateString() ? time.format(d) : shortDate(d)
}

/** "Today at 20:34", "Yesterday at 09:12" or "12 Oct at 09:12". */
export function formatSynced(ms: number): string {
  const d = new Date(ms)
  const yesterday = new Date()
  yesterday.setDate(yesterday.getDate() - 1)
  const dayLabel =
    d.toDateString() === new Date().toDateString()
      ? 'Today'
      : d.toDateString() === yesterday.toDateString()
        ? 'Yesterday'
        : shortDate(d)
  return `${dayLabel} at ${time.format(d)}`
}

export function formatFull(ms: number): string {
  return full.format(ms)
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
