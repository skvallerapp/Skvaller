import { SvelteSet } from 'svelte/reactivity'
import * as api from './api'
import { defaultSettings, idleStatus } from './defaults'
import type {
  Account,
  Article,
  Feed,
  Folder,
  Scope,
  Session,
  Settings,
  SyncStatus,
  View,
} from './types'

/** How long an article is open before it is marked read, with `markReadOn: 'delay'`. */
const MARK_READ_DELAY = 2000

/** How long `markRead` waits for more before saving, and how old a batch gets at most. */
const READ_SAVE_DELAY = 300
const READ_SAVE_MAX_AGE = 1500

/** All UI state. Everything comes from the local cache through `api`; nothing here
 * waits for the network. */
class Store {
  /** False until the session is known, so the app doesn't flash the sign-in screen. */
  ready = $state(false)
  /** The active account can sync. */
  signedIn = $state(false)
  accounts = $state.raw<Account[]>([])
  activeId = $state<string | null>(null)
  readonly account: Account | null = $derived(this.accounts.find((a) => a.id === this.activeId) ?? null)
  /** The active account is the local one, whose feeds and folders are managed here. */
  readonly isLocal: boolean = $derived(this.account?.kind === 'local')
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
  /** The newest list load. An older one resolves with it, once the list is shown. */
  #listLoad = { request: 0, done: Promise.resolve() }
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
      // Closing the window: what is marked gets its chance to be saved.
      window.addEventListener('pagehide', () => this.#saveRead())
      // From the Settings window's Reader View page.
      await api.onFeedsChanged((accountId) => {
        if (accountId === this.activeId)
          this.#loadLibrary().catch((e) => console.error('Loading the feeds failed', e))
      })
      await api.onAccountsChanged((session) =>
        this.#applySession(session).catch((e) => console.error('Showing the accounts failed', e)),
      )
      const session = await api.session()
      this.settings = session.settings
      await this.#applySession(session)
    } catch (e) {
      // Such as no keychain available. The sign-in screen shows it.
      this.status = { ...this.status, error: api.errorMessage(e) }
    }
    this.ready = true
  }

  async signIn(server: string, username: string, password: string) {
    await this.#applySession(await api.signIn(server, username, password))
  }

  async addLocalAccount() {
    await this.#applySession(await api.addLocalAccount())
  }

  /** The newest session taken in. */
  #revision = -1
  /** The latest sync status of each account, also those not showing. */
  #statuses = new Map<string, SyncStatus>()

  /** The active account's ID, for the commands. */
  #account(): string {
    return this.activeId ?? ''
  }

  /** Shows another account. */
  async switchAccount(id: string) {
    if (id === this.activeId) return
    await this.#applySession(await api.setActiveAccount(id))
  }

  /** Takes in the accounts, from the start, a sign-in or any window's change. Another
   * active account drops everything of the old one and loads its own. */
  async #applySession(session: Session) {
    // Settings come through `settings-changed`; an account change may predate one.
    if (session.revision < this.#revision) return
    this.#revision = session.revision
    const switched = session.activeId !== this.activeId
    this.accounts = session.accounts
    if (switched || session.signedIn !== this.signedIn) {
      this.#reset()
      this.activeId = session.activeId
      // A sync's status may have come while the session was on its way.
      this.status = (session.activeId && this.#statuses.get(session.activeId)) || session.status
      // A rejected sign-in that came meanwhile wins.
      this.signedIn = session.signedIn && !this.status.unauthorized
      if (this.signedIn) await this.#reload()
    }
  }

  /** Unread counts of every account, for the account switcher. */
  async refreshAccounts() {
    const revision = this.#revision
    const accounts = await api.accounts()
    // A session that came meanwhile is newer.
    if (revision === this.#revision) this.accounts = accounts
  }

  sync() {
    if (this.activeId) api.syncNow(this.activeId).catch((e) => console.error('Syncing failed', e))
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

  /** Resolves once the list is loaded. */
  select(scope: Scope): Promise<void> {
    this.scope = scope
    return this.#showList()
  }

  setView(view: View) {
    this.view = view
    void this.#showList()
  }

  /** The latest selection's list load. Resolves once it's listed, or replaced. */
  listShown: Promise<void> = Promise.resolve()

  #showList() {
    // The new list waits only for marks made so far to save.
    this.#saveRead()
    this.articleId = null
    this.html = null
    this.#resetFullText()
    return (this.listShown = this.#loadList())
  }

  open(id: string) {
    this.articleId = id
    this.html = null
    const request = ++this.#htmlRequest
    void api.articleHtml(this.#account(), id).then(
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
    clearTimeout(this.#markReadTimer)
    if (this.read.has(id)) return
    if (this.settings.markReadOn === 'open') this.toggleRead(id)
    // Stepping on before then, as with j and k, leaves it unread.
    else if (this.settings.markReadOn === 'delay')
      this.#markReadTimer = setTimeout(() => {
        if (this.articleId === id && !this.read.has(id) && this.settings.markReadOn === 'delay') this.toggleRead(id)
      }, MARK_READ_DELAY)
  }

  #markReadTimer: ReturnType<typeof setTimeout> | undefined

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
    this.#write((account) => api.setFeedReaderView(account, feedId, on))
    if (this.article?.feedId === feedId && this.readerView !== on) this.toggleReaderView()
  }

  // The local account's feeds and folders.

  /** Subscribes and shows the feed. Fetching it can take seconds; a failure throws. */
  async subscribe(url: string, folderId: string | null) {
    const session = this.#session
    const account = this.#account()
    await this.#writes
    const id = await api.subscribe(account, url, folderId)
    // Another account is showing now: the feed is in the one it was added to.
    if (session !== this.#session) return
    await this.#loadLibrary()
    await this.select({ kind: 'feed', id })
  }

  /** The folder with this title, or else a new one. Returns its ID. */
  async createFolder(title: string): Promise<string> {
    const session = this.#session
    const account = this.#account()
    await this.#writes
    const id = await api.createFolder(account, title)
    if (session === this.#session) await this.#loadLibrary()
    return id
  }

  unsubscribe(feedId: string) {
    this.feeds = this.feeds.filter((f) => f.id !== feedId)
    this.#write((account) => api.unsubscribe(account, feedId))
    this.#afterRemoving({ kind: 'feed', id: feedId })
  }

  renameFeed(feedId: string, title: string) {
    this.feeds = this.feeds.map((f) => (f.id === feedId ? { ...f, title } : f))
    this.#write((account) => api.renameFeed(account, feedId, title))
  }

  moveFeed(feedId: string, folderId: string | null) {
    this.feeds = this.feeds.map((f) => (f.id === feedId ? { ...f, folderId } : f))
    this.#write((account) => api.moveFeed(account, feedId, folderId))
    // A folder's list gains or loses the feed's articles.
    if (this.scope.kind === 'folder') void this.#loadList()
  }

  renameFolder(folderId: string, title: string) {
    this.folders = this.folders.map((f) => (f.id === folderId ? { ...f, title } : f))
    this.#write((account) => api.renameFolder(account, folderId, title))
  }

  deleteFolder(folderId: string) {
    this.folders = this.folders.filter((f) => f.id !== folderId)
    this.feeds = this.feeds.map((f) => (f.folderId === folderId ? { ...f, folderId: null } : f))
    this.#write((account) => api.deleteFolder(account, folderId))
    this.#afterRemoving({ kind: 'folder', id: folderId })
  }

  /** A removed feed or folder that is selected gives way to All. Any other list
   * reloads, as it may have shown the feed's articles. */
  #afterRemoving(removed: Scope) {
    if (this.scope.kind === removed.kind && 'id' in this.scope && 'id' in removed && this.scope.id === removed.id)
      void this.select({ kind: 'all' })
    else if (removed.kind === 'feed') void this.#loadList()
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
    void api.fullText(this.#account(), id, retry).then(
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
    // The user's own choice wins over a pending mark.
    clearTimeout(this.#markReadTimer)
    const article = this.visible.find((a) => a.id === id)
    const read = !this.read.has(id)
    if (read) this.read.add(id)
    else this.read.delete(id)
    if (article) this.unreadByFeed = adjusted(this.unreadByFeed, [article], read ? -1 : 1)
    this.#write((account) => api.setRead(account, [id], read))
  }

  /** Marks these listed articles read, the unread ones: at once on screen, and saved
   * together once the marking pauses, as while scrolling. */
  markRead(ids: string[]) {
    // A list on its way out marks nothing, as scrolling may go on after picking another.
    if (this.#selectionKey() !== this.#listKey) return
    const wanted = new Set(ids)
    const articles = this.visible.filter((a) => wanted.has(a.id) && !this.read.has(a.id))
    if (articles.length === 0) return
    this.unreadByFeed = adjusted(this.unreadByFeed, articles, -1)
    // The batch is a queued write from the start, so loads wait for it and changes made
    // meanwhile come after it; the timer only lets it go.
    // A long scroll saves as it goes, so loads behind it don't wait for its end.
    if (this.#readBatch && Date.now() - this.#readBatch.started > READ_SAVE_MAX_AGE) this.#saveRead()
    if (!this.#readBatch) {
      const batch = { ids: new Set<string>(), release: () => {}, started: Date.now() }
      const released = new Promise<void>((resolve) => (batch.release = resolve))
      this.#write(async (account) => {
        await released
        await api.setRead(account, [...batch.ids], true)
      })
      this.#readBatch = batch
    }
    for (const a of articles) {
      this.read.add(a.id)
      this.#readBatch.ids.add(a.id)
    }
    clearTimeout(this.#readSaveTimer)
    this.#readSaveTimer = setTimeout(() => this.#saveRead(), READ_SAVE_DELAY)
  }

  #readBatch: { ids: Set<string>; release: () => void; started: number } | null = null
  #readSaveTimer: ReturnType<typeof setTimeout> | undefined

  /** Lets the pending batch of marks save now. */
  #saveRead() {
    clearTimeout(this.#readSaveTimer)
    const batch = this.#readBatch
    this.#readBatch = null
    batch?.release()
  }

  /** Marks every article in the current list read. Unlike reading one by one, the
   * Unread view then empties at once, except for the article being read. */
  markAllRead() {
    const unread = this.visible.filter((a) => !this.read.has(a.id))
    if (unread.length === 0) return
    for (const a of unread) this.read.add(a.id)
    this.unreadByFeed = adjusted(this.unreadByFeed, unread, -1)
    this.#write((account) => api.setRead(account, unread.map((a) => a.id), true))
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
    this.#write((account) => api.setStarred(account, [id], starred))
  }

  /**
   * Saves a local change after the ones before it, so they reach the cache in order.
   * Then reloads the counts from the cache, also after a failure, so the optimistic
   * counts never stay wrong. The reload is not part of `#writes`: it waits for it.
   */
  #write(save: (account: string) => Promise<void>) {
    // Marks waiting to be saved go first, in the order they were made.
    this.#saveRead()
    this.#writeCount++
    // The account shown when the change was made, even if another shows by the time
    // it's saved.
    const account = this.#account()
    const saved = this.#writes.then(() => save(account)).catch((e) => console.error('Saving a change failed', e))
    this.#writes = saved
    void saved.then(() => this.#loadLibrary()).catch((e) => console.error('Loading the feeds failed', e))
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
    this.status = { ...idleStatus }
    // Marks not saved yet belong to the account that goes; their write named it.
    this.#saveRead()
  }

  #onStatus(status: SyncStatus) {
    this.#statuses.set(status.accountId, status)
    if (status.accountId !== this.activeId) return
    this.status = status
    if (status.unauthorized) this.signedIn = false
    else if (this.signedIn && !status.syncing && !status.error) {
      // The sidebar's feeds can change without the articles, as icons do. The list,
      // the costly load, only when the articles did.
      const loads = [this.#loadLibrary()]
      // After a sync, the Unread view keeps what was read since it opened (FR-7a), and
      // the Starred view what was unstarred.
      if (status.changed)
        loads.push(
          this.#loadList(
            (a) =>
              // The open article stays too: unread, it may not be marked yet.
              a.id === this.articleId || (this.view === 'unread' ? this.read.has(a.id) : !this.starred.has(a.id)),
          ),
        )
      Promise.all(loads).catch((e) => {
        this.#noteCorrupt(e)
        console.error('Reloading after a sync failed', e)
      })
    }
  }

  async #reload(keep?: (a: Article) => boolean) {
    try {
      await Promise.all([this.#loadLibrary(), this.#loadList(keep)])
    } catch (e) {
      this.#noteCorrupt(e)
      throw e
    }
  }

  /** A read that found the cache damaged offers to start it over, as a sync that does. */
  #noteCorrupt(e: unknown) {
    if ((e as { kind?: string } | null)?.kind === 'corrupt') this.status = { ...this.status, cacheBroken: true }
  }

  /** Starts the active account's damaged cache over. Its sync, which follows, brings the
   * articles back. */
  async rebuildCache() {
    const account = this.#account()
    await api.rebuildCache(account)
    if (account !== this.#account()) return
    this.status = { ...this.status, error: null, cacheBroken: false }
    await this.#reload()
  }

  async #loadLibrary() {
    const session = this.#session
    // Before waiting: a change queued during the wait outdates this load too.
    const writes = this.#writeCount
    await this.#writes
    const lib = await api.library(this.#account())
    // A change made meanwhile isn't in these counts; its own reload brings them.
    if (session !== this.#session || writes !== this.#writeCount) return
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
  #loadList(keep: (a: Article) => boolean = () => false): Promise<void> {
    const request = ++this.#listRequest
    this.#listLoad = { request, done: this.#fetchList(request, keep) }
    return this.#listLoad.done
  }

  async #fetchList(request: number, keep: (a: Article) => boolean): Promise<void> {
    const selection = { view: this.view, scope: $state.snapshot(this.scope) }
    const key = this.#selectionKey()
    const writes = this.#writeCount
    await this.#writes
    // Changes made while waiting, such as scroll marks: wait for them before fetching.
    if (writes !== this.#writeCount && request === this.#listRequest) return this.#loadList(keep)
    let list = await api.articles(this.#account(), selection)
    if (request !== this.#listRequest) {
      // Not after a sign-out, which drops loads without starting one.
      if (this.#listLoad.request > request) return this.#listLoad.done
      return
    }
    // A change made while this loaded isn't in it, and would be undone. Load again; the
    // next load waits for the change to be saved.
    if (writes !== this.#writeCount) return this.#loadList(keep)

    if (selection.view !== 'all' && key === this.#listKey) {
      const ids = new Set(list.map((a) => a.id))
      const kept = this.visible.filter((a) => !ids.has(a.id) && keep(a))
      if (kept.length > 0) {
        // Absent from the fresh list of all articles or of a feed, it is read (Unread view)
        // or unstarred (Starred view) now, whatever the local state says. From a folder's,
        // its feed may have moved instead: the local state stays.
        const unread = selection.view === 'unread'
        const implied = selection.scope.kind !== 'folder'
        const local = (a: Article) => ({
          ...a,
          read: (implied && unread) || this.read.has(a.id),
          starred: (!implied || unread) && this.starred.has(a.id),
        })
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
