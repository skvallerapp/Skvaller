// A stand-in for the Tauri commands, so the UI runs in a plain browser during
// `bun run dev`. It is loaded only in development and outside Tauri (see api.ts).
import { defaultSettings } from './defaults'
import type { Article, Feed, Folder, Library, Selection, Session, Settings, SyncStatus } from './types'

const folders: Folder[] = [
  { id: 'f-tech', title: 'Technology' },
  { id: 'f-design', title: 'Design' },
  { id: 'f-news', title: 'News' },
]

const feeds: Feed[] = [
  { id: 'hn', title: 'Hacker News', folderId: 'f-tech', subscribed: true, icon: null, readerView: false },
  { id: 'lwn', title: 'LWN.net', folderId: 'f-tech', subscribed: true, icon: null, readerView: true },
  { id: 'rust', title: 'This Week in Rust', folderId: 'f-tech', subscribed: true, icon: null, readerView: false },
  { id: 'daring', title: 'Daring Fireball', folderId: 'f-design', subscribed: true, icon: null, readerView: false },
  { id: 'sidebar', title: 'Sidebar', folderId: 'f-design', subscribed: true, icon: null, readerView: false },
  { id: 'bbc', title: 'BBC News — World', folderId: 'f-news', subscribed: true, icon: null, readerView: false },
  { id: 'xkcd', title: 'xkcd', folderId: null, subscribed: true, icon: null, readerView: false },
  { id: 'kottke', title: 'kottke.org', folderId: null, subscribed: true, icon: null, readerView: false },
]

// 5,000 articles, to test scrolling. Deterministic, so reloads look the same.
// Stand-in favicons: a coloured tile with a letter. Three feeds have none, for the fallback.
const icon = (bg: string, letter: string) =>
  'data:image/svg+xml,' +
  encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" fill="${bg}"/>` +
      `<text x="16" y="23" font-family="sans-serif" font-size="20" font-weight="700" fill="#fff" text-anchor="middle">${letter}</text></svg>`,
  )
const icons: Record<string, string> = {
  hn: icon('#ff6600', 'Y'),
  lwn: icon('#222', 'L'),
  rust: icon('#b7410e', 'R'),
  daring: icon('#4a525a', '★'),
  bbc: icon('#bb1919', 'B'),
}
for (const f of feeds) f.icon = icons[f.id] ?? null

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

// Events reach every window of the app, like Tauri's, through a BroadcastChannel.
// Settings live in localStorage, so the Settings window and the main window share them.
type Handler = (payload: unknown) => void
const handlers = new Map<string, Set<Handler>>()
const channel = new BroadcastChannel('skvaller-mock')
channel.onmessage = (e) => handlers.get(e.data.event)?.forEach((h) => h(e.data.payload))

function emit(event: string, payload: unknown) {
  handlers.get(event)?.forEach((h) => h(payload))
  channel.postMessage({ event, payload })
}

export async function listen(event: string, handler: Handler): Promise<() => void> {
  if (!handlers.has(event)) handlers.set(event, new Set())
  handlers.get(event)!.add(handler)
  return () => handlers.get(event)!.delete(handler)
}

const SETTINGS_KEY = 'skvaller.mock.settings'
const SIGNED_OUT_KEY = 'skvaller.mock.signedOut'
function settings(): Settings {
  try {
    return { ...defaultSettings, ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? '{}') }
  } catch {
    return defaultSettings
  }
}

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
  status.syncing = true
  emit('sync-status', { ...status })
  setTimeout(() => {
    status.syncing = false
    status.lastSync = Date.now()
    emit('sync-status', { ...status })
  }, 1200)
}

type Args = Record<string, unknown>

const commands: Record<string, (args: Args) => unknown> = {
  session: (): Session => ({
    account: { server: 'https://rss.example.com/api/greader.php', username: 'demo' },
    signedIn: localStorage.getItem(SIGNED_OUT_KEY) === null,
    status: { ...status },
    settings: settings(),
  }),
  sign_in: () => {
    localStorage.removeItem(SIGNED_OUT_KEY)
    const session = commands.session({}) as Session
    emit('signed-in', session.account)
    return session
  },
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
  // Takes a moment, like fetching a page. xkcd's pages have nothing readable.
  full_text: async ({ id }) => {
    await new Promise((r) => setTimeout(r, 800))
    const a = articles.find((x) => x.id === id)
    if (!a || a.feedId === 'xkcd') throw { kind: 'other', message: 'Nothing to show in Reader View' }
    return (
      `<p>${a.summary}</p><p>This is the <strong>Reader View</strong> of the article: the full text ` +
      'extracted from its web page, with more paragraphs than the feed sent.</p>' +
      '<h2>The rest of the story</h2><p>Second paragraph of the full article.</p>' +
      '<p>Third paragraph, with a <a href="https://example.com/more">link</a>.</p>'
    )
  },
  set_feed_reader_view: ({ feedId, on }) => {
    const feed = feeds.find((f) => f.id === feedId)
    if (feed) feed.readerView = on as boolean
  },
  set_read: ({ ids, read }) => update(ids as string[], { read: read as boolean }),
  set_starred: ({ ids, starred }) => update(ids as string[], { starred: starred as boolean }),
  set_settings: ({ settings }) => {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings))
    emit('settings-changed', settings)
  },
  sign_out: () => {
    localStorage.setItem(SIGNED_OUT_KEY, '1')
    emit('signed-out', null)
    window.close()
  },
  open_settings: () => void window.open('/settings.html', 'settings', 'width=720,height=520'),
}

export async function invoke<T>(cmd: string, args: Args = {}): Promise<T> {
  const command = commands[cmd]
  if (!command) throw { kind: 'other', message: `Mock has no command "${cmd}"` }
  return command(args) as T
}
