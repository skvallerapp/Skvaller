// A stand-in for the Tauri commands, so the UI runs in a plain browser during
// `bun run dev`. It is loaded only in development and outside Tauri (see api.ts).
import { defaultSettings } from './defaults'
import type { Account, Article, Feed, Folder, Library, Selection, Session, Settings, SyncStatus } from './types'

const allFolders: Folder[] = [
  { id: 'f-tech', title: 'Technology' },
  { id: 'f-design', title: 'Design' },
  { id: 'f-news', title: 'News' },
]

const allFeeds: Feed[] = [
  { id: 'hn', title: 'Hacker News', folderId: 'f-tech' },
  { id: 'lwn', title: 'LWN.net', folderId: 'f-tech', readerView: true },
  { id: 'rust', title: 'This Week in Rust', folderId: 'f-tech' },
  { id: 'daring', title: 'Daring Fireball', folderId: 'f-design' },
  { id: 'sidebar', title: 'Sidebar', folderId: 'f-design' },
  { id: 'bbc', title: 'BBC News — World', folderId: 'f-news' },
  { id: 'xkcd', title: 'xkcd', folderId: null },
  { id: 'kottke', title: 'kottke.org', folderId: null },
].map((f) => ({ subscribed: true, icon: null, readerView: false, url: null, siteUrl: null, fetchError: null, ...f }))

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
for (const f of allFeeds) f.icon = icons[f.id] ?? null

/** For a local account: feeds with addresses, one failing. */
const localFeeds = (feeds: Feed[]): Feed[] =>
  feeds.map((f, i) => ({
    ...f,
    url: `https://${f.id}.example/feed.xml`,
    siteUrl: `https://${f.id}.example/`,
    fetchError: i === 1 ? 'the server answered 404 Not Found' : null,
  }))

const now = Date.now()

interface Data {
  folders: Folder[]
  feeds: Feed[]
  articles: Article[]
}

/** Each account's feeds and articles. The first has them all; others have some, so
 * switching shows a difference. */
const data = new Map<string, Data>()
function dataOf(id: string): Data {
  let d = data.get(id)
  if (!d) {
    const first = id === '1'
    let feeds = first ? allFeeds : allFeeds.filter((_, i) => i % 2 === 1).map((f) => ({ ...f }))
    if (saved().accounts.find((a) => a.id === id)?.kind === 'local') feeds = localFeeds(feeds)
    const folders = allFolders.filter((f) => feeds.some((feed) => feed.folderId === f.id))
    d = { folders, feeds, articles: makeArticles(feeds, first ? 5000 : 300, Number(id) * 100_000) }
    data.set(id, d)
  }
  return d
}

// 5,000 articles for the first account, to test scrolling. Deterministic, so reloads
// look the same.
const makeArticles = (feeds: Feed[], count: number, idBase: number): Article[] =>
  Array.from({ length: count }, (_, i) => {
  const feed = feeds[i % feeds.length]
  return {
    id: String(idBase + 1_000_000 + i),
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

const statuses = new Map<string, SyncStatus>()
function statusOf(id: string): SyncStatus {
  let s = statuses.get(id)
  if (!s) statuses.set(id, (s = { accountId: id, syncing: false, lastSync: now - 5 * 60_000, error: null, unauthorized: false }))
  return s
}

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
/** `{ accounts, active }`, as config.json has them. Missing: one FreshRSS account. */
const ACCOUNTS_KEY = 'skvaller.mock.accounts'
const PANE_KEY = 'skvaller.mock.settingsPane'

// Omit on each member of the union, so `kind` still tells them apart.
type SavedAccount = Account extends infer A ? (A extends Account ? Omit<A, 'unread'> : never) : never
interface SavedAccounts {
  accounts: SavedAccount[]
  active: string | null
}

const demoAccount: SavedAccount = {
  id: '1',
  description: 'FreshRSS',
  kind: 'greader',
  server: 'https://rss.example.com/api/greader.php',
  username: 'demo',
  sync: 15,
  keepReadDays: 30,
}

function saved(): SavedAccounts {
  try {
    const value = localStorage.getItem(ACCOUNTS_KEY)
    if (value) return JSON.parse(value)
  } catch {
    // The default below.
  }
  return { accounts: [demoAccount], active: '1' }
}

function save(value: SavedAccounts) {
  localStorage.setItem(ACCOUNTS_KEY, JSON.stringify(value))
  emit('accounts-changed', session())
}

/** The account the command being run names, else the active one. */
let commandAccount: string | null = null

function active(): Data {
  return dataOf(commandAccount ?? saved().active ?? '1')
}

function accounts(): Account[] {
  return saved().accounts.map((a) => ({ ...a, unread: dataOf(a.id).articles.filter((x) => !x.read).length }) as Account)
}

function session(): Session {
  const { active } = saved()
  return {
    revision: Date.now(),
    accounts: accounts(),
    activeId: active,
    signedIn: active !== null,
    status: active ? { ...statusOf(active) } : { accountId: '', syncing: false, lastSync: null, error: null, unauthorized: false },
    settings: settings(),
  }
}
function settings(): Settings {
  try {
    return { ...defaultSettings, ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? '{}') }
  } catch {
    return { ...defaultSettings }
  }
}

function matches(a: Article, { view, scope }: Selection, feeds: Feed[]): boolean {
  if (view === 'unread' ? a.read : view === 'starred' && !a.starred) return false
  switch (scope.kind) {
    case 'all':
      return true
    case 'folder':
      return feeds.some((f) => f.id === a.feedId && f.folderId === scope.id)
    case 'feed':
      return a.feedId === scope.id
  }
}

function byFeed(articles: Article[], keep: (a: Article) => boolean): Record<string, number> {
  const counts: Record<string, number> = {}
  for (const a of articles) if (keep(a)) counts[a.feedId] = (counts[a.feedId] ?? 0) + 1
  return counts
}

function library(): Library {
  const { folders, feeds, articles } = active()
  const counts = {
    unreadByFeed: byFeed(articles, (a) => !a.read),
    starredByFeed: byFeed(articles, (a) => a.starred),
  }
  // Copies, as from Tauri: the store sees a new list as a change.
  return { folders: folders.map((f) => ({ ...f })), feeds: feeds.map((f) => ({ ...f })), counts }
}

function update(ids: string[], change: Partial<Article>) {
  const set = new Set(ids)
  for (const a of active().articles) if (set.has(a.id)) Object.assign(a, change)
}

function sync() {
  const id = commandAccount ?? saved().active
  if (!id) return
  const status = statusOf(id)
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
  session,
  accounts,
  sign_in: ({ server, username }) => {
    const value = saved()
    let account = value.accounts.find((a) => a.kind === 'greader' && a.server === server && a.username === username)
    if (!account) {
      const id = String(Math.max(0, ...value.accounts.map((a) => Number(a.id))) + 1)
      account = { ...demoAccount, id, server: server as string, username: username as string, description: new URL(server as string).hostname }
      value.accounts.push(account)
    }
    value.active = account.id
    save(value)
    return session()
  },
  add_local_account: () => {
    const value = saved()
    const id = String(Math.max(0, ...value.accounts.map((a) => Number(a.id))) + 1)
    value.accounts.push({ id, description: 'On This Device', kind: 'local', sync: 15, keepReadDays: 30 })
    value.active = id
    save(value)
    return session()
  },
  // Takes a moment, like fetching the feed. An address with "nofeed" in it has none.
  subscribe: async ({ url, folderId }) => {
    const d = active()
    await new Promise((r) => setTimeout(r, 600))
    if (String(url).includes('nofeed')) throw { kind: 'noFeed', message: 'no feed found at this address' }
    const id = `feed/${url}`
    if (!d.feeds.some((f) => f.id === id)) {
      const title = String(url).replace(/^https?:\/\//, '').split('/')[0]
      d.feeds.push({ id, title, folderId: (folderId as string | null) ?? null, subscribed: true, icon: null, readerView: false, url: String(url), siteUrl: null, fetchError: null })
      d.articles.unshift(...makeArticles([d.feeds.at(-1)!], 5, 50_000_000 + (Date.now() % 1_000_000) * 10))
    }
    return id
  },
  unsubscribe: ({ feedId }) => {
    const d = active()
    d.feeds = d.feeds.filter((f) => f.id !== feedId)
    d.articles = d.articles.filter((a) => a.feedId !== feedId)
  },
  rename_feed: ({ feedId, title }) => {
    const feed = active().feeds.find((f) => f.id === feedId)
    if (feed) feed.title = title as string
  },
  move_feed: ({ feedId, folderId }) => {
    const feed = active().feeds.find((f) => f.id === feedId)
    if (feed) feed.folderId = folderId as string | null
  },
  create_folder: ({ title }) => {
    const d = active()
    const existing = d.folders.find((f) => f.title.toLowerCase() === String(title).toLowerCase())
    if (existing) return existing.id
    const id = `folder/${d.folders.length + 1}`
    d.folders = [...d.folders, { id, title: title as string }]
    return id
  },
  rename_folder: ({ folderId, title }) => {
    const d = active()
    d.folders = d.folders.map((f) => (f.id === folderId ? { ...f, title: title as string } : f))
  },
  delete_folder: ({ folderId }) => {
    const d = active()
    d.folders = d.folders.filter((f) => f.id !== folderId)
    for (const f of d.feeds) if (f.folderId === folderId) f.folderId = null
  },
  import_opml: () => ({ added: 3, skipped: 1 }),
  export_opml: () => true,
  set_active_account: ({ id }) => {
    save({ ...saved(), active: id as string })
    return session()
  },
  update_account: ({ id, description, sync, keepReadDays }) => {
    const value = saved()
    const change = Object.fromEntries(
      Object.entries({ description, sync, keepReadDays }).filter(([, v]) => v !== undefined),
    )
    value.accounts = value.accounts.map((a) => (a.id === id ? { ...a, ...change } : a))
    save(value)
  },
  remove_account: ({ id }) => {
    const value = saved()
    value.accounts = value.accounts.filter((a) => a.id !== id)
    if (value.active === id) value.active = value.accounts[0]?.id ?? null
    data.delete(id as string)
    save(value)
    if (value.accounts.length === 0) window.close()
  },
  sync_now: () => sync(),
  library,
  articles: ({ selection }) => {
    const { articles, feeds } = active()
    return articles.filter((a) => matches(a, selection as Selection, feeds)).map((a) => ({ ...a }))
  },
  article_html: ({ id }) => {
    const a = active().articles.find((x) => x.id === id)
    return a
      ? `<p>${a.summary}</p><h2>A subheading</h2><p>Mock article body for <a href="${a.url}">${a.title}</a>.</p>` +
          '<blockquote>A quoted passage.</blockquote><pre><code>let x = 42;</code></pre>'
      : null
  },
  // Takes a moment, like fetching a page. xkcd's pages have nothing readable.
  full_text: async ({ id }) => {
    const { articles } = active()
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
    const feed = active().feeds.find((f) => f.id === feedId)
    if (feed) feed.readerView = on as boolean
  },
  set_read: ({ ids, read }) => update(ids as string[], { read: read as boolean }),
  set_starred: ({ ids, starred }) => update(ids as string[], { starred: starred as boolean }),
  set_settings: ({ settings }) => {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings))
    emit('settings-changed', settings)
  },
  open_settings: ({ pane }) => {
    if (pane) {
      localStorage.setItem(PANE_KEY, pane as string)
      emit('settings-pane', pane)
    }
    window.open('/settings.html', 'settings', 'width=720,height=520')
  },
  take_settings_pane: () => {
    const pane = localStorage.getItem(PANE_KEY)
    localStorage.removeItem(PANE_KEY)
    return pane
  },
}

export async function invoke<T>(cmd: string, args: Args = {}): Promise<T> {
  const command = commands[cmd]
  if (!command) throw { kind: 'other', message: `Mock has no command "${cmd}"` }
  commandAccount = (args.accountId as string | undefined) ?? null
  try {
    return command(args) as T
  } finally {
    commandAccount = null
  }
}
