// A stand-in for the Tauri commands, so the UI runs in a plain browser during
// `bun run dev`. It is loaded only in development and outside Tauri (see api.ts).
import type { Article, Feed, Folder, Library, Selection, Session, SyncStatus } from './types'

const folders: Folder[] = [
  { id: 'f-tech', title: 'Technology' },
  { id: 'f-design', title: 'Design' },
  { id: 'f-news', title: 'News' },
]

const feeds: Feed[] = [
  { id: 'hn', title: 'Hacker News', folderId: 'f-tech', subscribed: true },
  { id: 'lwn', title: 'LWN.net', folderId: 'f-tech', subscribed: true },
  { id: 'rust', title: 'This Week in Rust', folderId: 'f-tech', subscribed: true },
  { id: 'daring', title: 'Daring Fireball', folderId: 'f-design', subscribed: true },
  { id: 'sidebar', title: 'Sidebar', folderId: 'f-design', subscribed: true },
  { id: 'bbc', title: 'BBC News — World', folderId: 'f-news', subscribed: true },
  { id: 'xkcd', title: 'xkcd', folderId: null, subscribed: true },
  { id: 'kottke', title: 'kottke.org', folderId: null, subscribed: true },
]

// 5,000 articles, to test scrolling. Deterministic, so reloads look the same.
const now = Date.now()
const articles: Article[] = Array.from({ length: 5000 }, (_, i) => {
  const feed = feeds[i % feeds.length]
  return {
    id: String(1_000_000 + i),
    feedId: feed.id,
    title: `${feed.title} article ${5000 - i}: a headline long enough to wrap onto a second line`,
    author: i % 3 === 0 ? '' : `Author ${i % 17}`,
    url: `https://example.com/${feed.id}/${i}`,
    published: now - i * 17 * 60_000,
    summary:
      'A plain-text preview of the article. It runs for a sentence or two so the list shows how summaries are clipped.',
    read: i % 5 !== 0,
    starred: i % 23 === 0,
  }
})

const status: SyncStatus = { syncing: false, lastSync: now - 5 * 60_000, error: null, unauthorized: false }
const statusHandlers: ((s: SyncStatus) => void)[] = []

function matches(a: Article, sel: Selection): boolean {
  switch (sel.kind) {
    case 'smart':
      return sel.view === 'unread' ? !a.read : sel.view === 'starred' ? a.starred : true
    case 'folder':
      return feeds.some((f) => f.id === a.feedId && f.folderId === sel.id)
    case 'feed':
      return a.feedId === sel.id
  }
}

function library(): Library {
  const unreadByFeed: Record<string, number> = {}
  for (const a of articles) if (!a.read) unreadByFeed[a.feedId] = (unreadByFeed[a.feedId] ?? 0) + 1
  return { folders, feeds, counts: { unreadByFeed, starred: articles.filter((a) => a.starred).length } }
}

function update(ids: string[], change: Partial<Article>) {
  const set = new Set(ids)
  for (const a of articles) if (set.has(a.id)) Object.assign(a, change)
}

function sync() {
  const emit = () => statusHandlers.forEach((h) => h({ ...status }))
  status.syncing = true
  emit()
  setTimeout(() => {
    status.syncing = false
    status.lastSync = Date.now()
    emit()
  }, 1200)
}

type Args = Record<string, unknown>

const commands: Record<string, (args: Args) => unknown> = {
  session: (): Session => ({
    account: { server: 'https://rss.example.com/api/greader.php', username: 'demo' },
    signedIn: true,
    status: { ...status },
    settings: { confirmMarkAllRead: true },
  }),
  sign_in: () => commands.session({}),
  sync_now: () => sync(),
  library,
  articles: ({ selection }) => articles.filter((a) => matches(a, selection as Selection)).map((a) => ({ ...a })),
  article_html: ({ id }) => {
    const a = articles.find((x) => x.id === id)
    return a
      ? `<p>${a.summary}</p><h2>A subheading</h2><p>Mock article body for <a href="${a.url}">${a.title}</a>.</p>` +
          '<blockquote>A quoted passage.</blockquote><pre><code>let x = 42;</code></pre>'
      : null
  },
  set_read: ({ ids, read }) => update(ids as string[], { read: read as boolean }),
  set_starred: ({ ids, starred }) => update(ids as string[], { starred: starred as boolean }),
}

export async function invoke<T>(cmd: string, args: Args = {}): Promise<T> {
  const command = commands[cmd]
  if (!command) throw { kind: 'other', message: `Mock has no command "${cmd}"` }
  return command(args) as T
}

export async function onSyncStatus(handler: (s: SyncStatus) => void): Promise<() => void> {
  statusHandlers.push(handler)
  return () => statusHandlers.splice(statusHandlers.indexOf(handler), 1)
}
