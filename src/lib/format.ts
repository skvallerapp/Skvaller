const time = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' })
const day = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' })
const full = new Intl.DateTimeFormat(undefined, { dateStyle: 'long', timeStyle: 'short' })

/** Time for today, a short date otherwise. */
export function formatTime(ms: number): string {
  const d = new Date(ms)
  return d.toDateString() === new Date().toDateString() ? time.format(d) : day.format(d)
}

export function formatFull(ms: number): string {
  return full.format(ms)
}

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`)
}
