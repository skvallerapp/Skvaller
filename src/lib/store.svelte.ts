import { SvelteSet } from 'svelte/reactivity'
import * as api from './api'
import { defaultSettings, idleStatus } from './defaults'
import type {
  Account,
  Article,
  Feed,
  Folder,
  Scope,
  Settings,
  SyncStatus,
  View,
} from './types'

/** All UI state. Everything comes from the local cache through `api`; nothing here
 * waits for the network. */
class Store {
  /** False until the session is known, so the app doesn't flash the sign-in screen. */
  ready = $state(false)
  signedIn = $state(false)
  account = $state<Account | null>(null)
  status = $state<SyncStatus>({ ...idleStatus })
  /** Until `init` loads them. */
  settings = $state<Settings>({ ...defaultSettings })

  folders = $state.raw<Folder[]>([])
  feeds = $state.raw<Feed[]>([])
  unreadByFeed = $state.raw(new Map<string, number>())
  starredByFeed = $state.raw(new Map<string, number>())

  /** The list shows the articles in `view` from `scope`. */
  view = $state<View>('unread')
  scope = $state<Scope>({ kind: 'all' })
  visible = $state.raw<Article[]>([])
  articleId = $state<string | null>(null)
  /** Untrusted HTML of the open article. Render only in the sandboxed reader frame. */
  html = $state<string | null>(null)
  /** The open article shows Reader View: its full text, from its web page. */
  readerView = $state(false)
  /** Untrusted Reader View HTML of the open article, once loaded. */
  fullText = $state<string | null>(null)
  fullTextState = $state<'idle' | 'loading' | 'failed'>('idle')
  /** For a moment after Reader View starts loading, the reader waits for it instead of
   * showing the feed's content and swapping. */
  awaitingFullText = $state(false)

  /** Read and starred state of the listed articles, changed in place by the user. */
  readonly read = new SvelteSet<string>()
  readonly starred = new SvelteSet<string>()

  #feedById = $derived(new Map(this.feeds.map((f) => [f.id, f])))
  // Responses to older requests are dropped when a newer one was made.
  #listRequest = 0
  /** The selection `visible` was loaded for, as JSON. */
  #listKey = ''
  #htmlRequest = 0
  #fullTextRequest = 0
  // Bumped on sign-out, so loads from before it are dropped.
  #session = 0
  // Local changes, saved one after another. Reads wait for it, so they never see the
  // cache from before a change.
  #writes: Promise<void> = Promise.resolve()
  /** Changes made so far. A list load that sees it change started too early. */
  #writeCount = 0

  /** Feeds to show in the sidebar. Unsubscribed ones exist only for old starred articles. */
  readonly sidebarFeeds: Feed[] = $derived(this.feeds.filter((f) => f.subscribed))

  /** The counts the sidebar shows: starred ones in the Starred view, else unread ones. */
  readonly counts: Map<string, number> = $derived(
    this.view === 'starred' ? this.starredByFeed : this.unreadByFeed,
  )

  readonly countTotal: number = $derived([...this.counts.values()].reduce((sum, n) => sum + n, 0))

  /** Unread articles in the current list. */
  readonly unreadInView: number = $derived(
    this.visible.reduce((n, a) => (this.read.has(a.id) ? n : n + 1), 0),
  )

  readonly article: Article | null = $derived(
    this.visible.find((a) => a.id === this.articleId) ?? null,
  )

  async init() {
    try {
      // Listen first, so a sync that finishes meanwhile is not missed.
      await api.onSyncStatus((status) => this.#onStatus(status))
      await api.onSettingsChanged((settings) => (this.settings = settings))
      await api.onSignedOut(() => this.#onSignedOut())
      const session = await api.session()
      this.account = session.account
      this.status = session.status
      this.settings = session.settings
      this.signedIn = session.signedIn
      if (this.signedIn) await this.#reload()
    } catch (e) {
      // Such as no keychain available. The sign-in screen shows it.
      this.status = { ...this.status, error: api.errorMessage(e) }
    }
    this.ready = true
  }

  async signIn(server: string, username: string, password: string) {
    const session = await api.signIn(server, username, password)
    // After a rejected token, maybe as someone else: nothing of the old session stays.
    this.#reset()
    this.account = session.account
    this.status = session.status
    this.settings = session.settings
    this.signedIn = true
    await this.#reload()
  }

  sync() {
    void api.syncNow()
  }

  feed(id: string): Feed | undefined {
    return this.#feedById.get(id)
  }

  /** `counts` summed per folder. */
  readonly folderCounts: Map<string, number> = $derived.by(() => {
    const sums = new Map<string, number>()
    for (const f of this.feeds)
      if (f.folderId !== null) sums.set(f.folderId, (sums.get(f.folderId) ?? 0) + (this.counts.get(f.id) ?? 0))
    return sums
  })

  select(scope: Scope) {
    this.scope = scope
    this.#showList()
  }

  setView(view: View) {
    this.view = view
    this.#showList()
  }

  #showList() {
    this.articleId = null
    this.html = null
    this.#resetFullText()
    void this.#loadList()
  }

  open(id: string) {
    this.articleId = id
    this.html = null
    const request = ++this.#htmlRequest
    void api.articleHtml(id).then(
      (html) => {
        if (request === this.#htmlRequest) this.html = html ?? ''
      },
      (e) => {
        console.error('Loading the article failed', e)
        if (request === this.#htmlRequest) this.html = ''
      },
    )
    if (id !== this.#fullTextId) {
      this.#resetFullText()
      this.#fullTextId = id
      const article = this.visible.find((a) => a.id === id)
      if (article?.url && this.feed(article.feedId)?.readerView) {
        this.readerView = true
        this.#loadFullText(id, { retry: false, hold: true })
      }
    }
    if (!this.read.has(id)) this.toggleRead(id)
  }

  /** Switches the open article between Reader View and the feed's content. */
  toggleReaderView() {
    if (!this.article?.url) return
    this.readerView = !this.readerView
    if (this.readerView && this.fullText === null && this.fullTextState !== 'loading')
      this.#loadFullText(this.article.id, { retry: true, hold: false })
  }

  /** Opens the feed's articles in Reader View from now on, or stops. The open article
   * follows. */
  setFeedReaderView(feedId: string, on: boolean) {
    this.feeds = this.feeds.map((f) => (f.id === feedId ? { ...f, readerView: on } : f))
    this.#write(() => api.setFeedReaderView(feedId, on))
    if (this.article?.feedId === feedId && this.readerView !== on) this.toggleReaderView()
  }

  /** Fetching can take seconds. Until it's done, the reader shows the feed's content;
   * with `hold`, for an article just opened, only after a moment. If it fails, Reader
   * View turns off and says so. `retry` fetches a page given up on. */
  #loadFullText(id: string, { retry, hold }: { retry: boolean; hold: boolean }) {
    const request = ++this.#fullTextRequest
    this.fullTextState = 'loading'
    this.awaitingFullText = hold
    if (hold)
      setTimeout(() => {
        if (request === this.#fullTextRequest) this.awaitingFullText = false
      }, 400)
    void api.fullText(id, retry).then(
      (html) => {
        if (request !== this.#fullTextRequest) return
        this.fullText = html
        this.fullTextState = 'idle'
        this.awaitingFullText = false
      },
      (e) => {
        if (request !== this.#fullTextRequest) return
        console.warn('Reader View failed', e)
        // Turned off meanwhile: nothing to say.
        this.fullTextState = this.readerView ? 'failed' : 'idle'
        this.awaitingFullText = false
        this.readerView = false
      },
    )
  }

  /** The article whose Reader View state this is. */
  #fullTextId: string | null = null

  #resetFullText() {
    this.#fullTextRequest++
    this.#fullTextId = null
    this.fullText = null
    this.fullTextState = 'idle'
    this.awaitingFullText = false
    this.readerView = false
  }

  /** Opens the next (1) or previous (-1) article in the list. With none open, opens the
   * first. */
  step(delta: 1 | -1) {
    // Until a new selection's list arrives, `visible` is the old one.
    if (this.#selectionKey() !== this.#listKey) return
    const i = this.visible.findIndex((a) => a.id === this.articleId)
    const next = this.visible[i === -1 ? 0 : i + delta]
    if (next) this.open(next.id)
  }

  toggleRead(id: string) {
    const article = this.visible.find((a) => a.id === id)
    const read = !this.read.has(id)
    if (read) this.read.add(id)
    else this.read.delete(id)
    if (article) this.unreadByFeed = adjusted(this.unreadByFeed, [article], read ? -1 : 1)
    this.#write(() => api.setRead([id], read))
  }

  /** Marks every article in the current list read. Unlike reading one by one, the
   * Unread view then empties at once, except for the article being read. */
  markAllRead() {
    const unread = this.visible.filter((a) => !this.read.has(a.id))
    if (unread.length === 0) return
    for (const a of unread) this.read.add(a.id)
    this.unreadByFeed = adjusted(this.unreadByFeed, unread, -1)
    this.#write(() => api.setRead(unread.map((a) => a.id), true))
    const open = this.articleId
    const keepOpen = (a: Article) => a.id === open
    if (this.view === 'unread') this.visible = this.visible.filter(keepOpen)
    void this.#loadList(keepOpen)
  }

  toggleStarred(id: string) {
    const article = this.visible.find((a) => a.id === id)
    const starred = !this.starred.has(id)
    if (starred) this.starred.add(id)
    else this.starred.delete(id)
    if (article) this.starredByFeed = adjusted(this.starredByFeed, [article], starred ? 1 : -1)
    this.#write(() => api.setStarred([id], starred))
  }

  /**
   * Saves a local change after the ones before it, so they reach the cache in order.
   * Then reloads the counts from the cache, also after a failure, so the optimistic
   * counts never stay wrong. The reload is not part of `#writes`: it waits for it.
   */
  #write(save: () => Promise<void>) {
    this.#writeCount++
    const saved = this.#writes.then(save).catch((e) => console.error('Saving a change failed', e))
    this.#writes = saved
    void saved.then(() => this.#loadLibrary()).catch((e) => console.error('Loading the feeds failed', e))
  }

  /** Signing out happens in the Settings window; the cache is empty by now. */
  #onSignedOut() {
    this.#reset()
    this.signedIn = false
    this.account = null
    this.status = { ...idleStatus }
  }

  /** Forgets the account's feeds and articles, and drops responses still on the way. */
  #reset() {
    this.#session++
    this.#listRequest++
    this.#htmlRequest++
    this.#listKey = ''
    this.folders = []
    this.feeds = []
    this.unreadByFeed = new Map()
    this.starredByFeed = new Map()
    this.view = 'unread'
    this.scope = { kind: 'all' }
    this.visible = []
    this.articleId = null
    this.html = null
    this.#resetFullText()
    this.read.clear()
    this.starred.clear()
  }

  #onStatus(status: SyncStatus) {
    this.status = status
    if (status.unauthorized) this.signedIn = false
    // After a sync, the Unread view keeps what was read since it opened (FR-7a), and the
    // Starred view what was unstarred.
    else if (this.signedIn && !status.syncing && !status.error)
      this.#reload((a) => (this.view === 'unread' ? this.read.has(a.id) : !this.starred.has(a.id))).catch((e) =>
        console.error('Reloading after a sync failed', e),
      )
  }

  async #reload(keep?: (a: Article) => boolean) {
    await Promise.all([this.#loadLibrary(), this.#loadList(keep)])
  }

  async #loadLibrary() {
    const session = this.#session
    await this.#writes
    const lib = await api.library()
    if (session !== this.#session) return
    this.folders = lib.folders
    this.feeds = lib.feeds
    this.unreadByFeed = new Map(Object.entries(lib.counts.unreadByFeed))
    this.starredByFeed = new Map(Object.entries(lib.counts.starredByFeed))
  }

  #selectionKey(): string {
    return JSON.stringify({ view: this.view, scope: this.scope })
  }

  /**
   * Loads the list for the current selection. In the Unread and Starred views, listed
   * articles that left the view stay if `keep` says so, so the list doesn't jump while
   * you read. Only a reload of the same selection keeps any.
   */
  async #loadList(keep: (a: Article) => boolean = () => false): Promise<void> {
    const request = ++this.#listRequest
    const selection = { view: this.view, scope: $state.snapshot(this.scope) }
    const key = this.#selectionKey()
    const writes = this.#writeCount
    await this.#writes
    let list = await api.articles(selection)
    if (request !== this.#listRequest) return
    // A change made while this loaded isn't in it, and would be undone. Load again; the
    // next load waits for the change to be saved.
    if (writes !== this.#writeCount) return this.#loadList(keep)

    if (selection.view !== 'all' && key === this.#listKey) {
      const ids = new Set(list.map((a) => a.id))
      const kept = this.visible.filter((a) => !ids.has(a.id) && keep(a))
      if (kept.length > 0) {
        const local = (a: Article) => ({ ...a, read: this.read.has(a.id), starred: this.starred.has(a.id) })
        list = [...list, ...kept.map(local)]
        list.sort((a, b) => b.published - a.published)
      }
    }
    this.#listKey = key

    this.read.clear()
    this.starred.clear()
    for (const a of list) {
      if (a.read) this.read.add(a.id)
      if (a.starred) this.starred.add(a.id)
    }
    this.visible = list
  }
}

/** `counts` with each article's feed changed by `delta`, for the sidebar to show right
 * away. The next load from the cache corrects them. */
function adjusted(counts: Map<string, number>, articles: Article[], delta: number) {
  const next = new Map(counts)
  for (const a of articles) next.set(a.feedId, Math.max(0, (next.get(a.feedId) ?? 0) + delta))
  return next
}

export const store = new Store()
