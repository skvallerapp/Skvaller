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

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`)
}

/** "FreshRSS" for FreshRSS's API path, the host name for other servers. */
export function serviceTitle(server: string): string {
  try {
    const url = new URL(server)
    return url.pathname.endsWith('/api/greader.php') ? 'FreshRSS' : url.hostname
  } catch {
    return server
  }
}

/**
 * Completes what the user typed into an API URL: `https://` when no scheme is given,
 * and FreshRSS's `/api/greader.php` when there is no path.
 */
export function apiUrl(input: string): string {
  let s = input.trim()
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(s)) s = `https://${s}`
  try {
    const url = new URL(s)
    if (url.pathname === '/' || url.pathname === '') url.pathname = '/api/greader.php'
    return url.toString().replace(/\/$/, '')
  } catch {
    return s
  }
}
