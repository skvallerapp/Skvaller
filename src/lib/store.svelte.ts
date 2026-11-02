import { SvelteSet } from 'svelte/reactivity'
import * as api from './api'
import type {
  Account,
  Article,
  CommandError,
  Feed,
  Folder,
  Selection,
  Settings,
  SyncStatus,
} from './types'

/** All UI state. Everything comes from the local cache through `api`; nothing here
 * waits for the network. */
class Store {
  /** False until the session is known, so the app doesn't flash the sign-in screen. */
  ready = $state(false)
  signedIn = $state(false)
  account = $state<Account | null>(null)
  status = $state<SyncStatus>({ syncing: false, lastSync: null, error: null, unauthorized: false })
  settings = $state<Settings>({ confirmMarkAllRead: true, uiFont: 'inter', articleFont: 'inter' })

  folders = $state.raw<Folder[]>([])
  feeds = $state.raw<Feed[]>([])
  unreadByFeed = $state.raw(new Map<string, number>())
  starredCount = $state(0)

  selection = $state<Selection>({ kind: 'smart', view: 'unread' })
  visible = $state.raw<Article[]>([])
  articleId = $state<string | null>(null)
  /** Untrusted HTML of the open article. Render only in the sandboxed reader frame. */
  html = $state<string | null>(null)

  /** Read and starred state of the listed articles, changed in place by the user. */
  readonly read = new SvelteSet<string>()
  readonly starred = new SvelteSet<string>()

  #feedById = $derived(new Map(this.feeds.map((f) => [f.id, f])))
  // Responses to older requests are dropped when a newer one was made.
  #listRequest = 0
  #htmlRequest = 0
  // Local changes, saved one after another. Reads wait for it, so they never see the
  // cache from before a change.
  #writes: Promise<void> = Promise.resolve()

  /** Feeds to show in the sidebar. Unsubscribed ones exist only for old starred articles. */
  readonly sidebarFeeds: Feed[] = $derived(this.feeds.filter((f) => f.subscribed))

  readonly unreadTotal: number = $derived(
    [...this.unreadByFeed.values()].reduce((sum, n) => sum + n, 0),
  )

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
      const session = await api.session()
      this.account = session.account
      this.status = session.status
      this.settings = session.settings
      this.signedIn = session.signedIn
      if (this.signedIn) await this.#reload()
    } catch (e) {
      // Such as no keychain available. The sign-in screen shows it.
      this.status = { ...this.status, error: (e as CommandError).message ?? String(e) }
    }
    this.ready = true
  }

  async signIn(server: string, username: string, password: string) {
    const session = await api.signIn(server, username, password)
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

  folderUnread(folderId: string): number {
    let n = 0
    for (const f of this.feeds) if (f.folderId === folderId) n += this.unreadByFeed.get(f.id) ?? 0
    return n
  }

  select(selection: Selection) {
    this.selection = selection
    this.articleId = null
    this.html = null
    void this.#loadList()
  }

  open(id: string) {
    this.articleId = id
    this.html = null
    const request = ++this.#htmlRequest
    void api.articleHtml(id).then((html) => {
      if (request === this.#htmlRequest) this.html = html ?? ''
    })
    if (!this.read.has(id)) this.toggleRead(id)
  }

  /** Opens the next (1) or previous (-1) article in the list. With none open, opens the
   * first. */
  step(delta: 1 | -1) {
    const i = this.visible.findIndex((a) => a.id === this.articleId)
    const next = this.visible[i === -1 ? 0 : i + delta]
    if (next) this.open(next.id)
  }

  toggleRead(id: string) {
    const article = this.visible.find((a) => a.id === id)
    const read = !this.read.has(id)
    if (read) this.read.add(id)
    else this.read.delete(id)
    if (article) this.#adjustUnread([article], read ? -1 : 1)
    this.#write(() => api.setRead([id], read))
  }

  /** Marks every article in the current list read. Unlike reading one by one, the
   * Unread view then empties at once, except for the article being read. */
  markAllRead() {
    const unread = this.visible.filter((a) => !this.read.has(a.id))
    if (unread.length === 0) return
    for (const a of unread) this.read.add(a.id)
    this.#adjustUnread(unread, -1)
    this.#write(() => api.setRead(unread.map((a) => a.id), true))
    const open = this.articleId
    const keepOpen = (a: Article) => a.id === open
    if (this.#isUnreadView) this.visible = this.visible.filter(keepOpen)
    void this.#loadList(keepOpen)
  }

  get #isUnreadView(): boolean {
    return this.selection.kind === 'smart' && this.selection.view === 'unread'
  }

  /** Adjusts the sidebar counts right away; the next load from the cache corrects them. */
  #adjustUnread(articles: Article[], delta: number) {
    const counts = new Map(this.unreadByFeed)
    for (const a of articles) counts.set(a.feedId, Math.max(0, (counts.get(a.feedId) ?? 0) + delta))
    this.unreadByFeed = counts
  }

  toggleStarred(id: string) {
    const starred = !this.starred.has(id)
    if (starred) this.starred.add(id)
    else this.starred.delete(id)
    this.starredCount += starred ? 1 : -1
    this.#write(() => api.setStarred([id], starred))
  }

  /**
   * Saves a local change after the ones before it, so they reach the cache in order.
   * Then reloads the counts from the cache, also after a failure, so the optimistic
   * counts never stay wrong. The reload is not part of `#writes`: it waits for it.
   */
  #write(save: () => Promise<void>) {
    const saved = this.#writes.then(save).catch((e) => console.error('Saving a change failed', e))
    this.#writes = saved
    void saved.then(() => this.#loadLibrary())
  }

  #onStatus(status: SyncStatus) {
    this.status = status
    if (status.unauthorized) this.signedIn = false
    // After a sync, the Unread view keeps what was read since it opened (FR-7a).
    else if (!status.syncing && !status.error) void this.#reload((a) => this.read.has(a.id))
  }

  async #reload(keep?: (a: Article) => boolean) {
    await Promise.all([this.#loadLibrary(), this.#loadList(keep)])
  }

  async #loadLibrary() {
    await this.#writes
    const lib = await api.library()
    this.folders = lib.folders
    this.feeds = lib.feeds
    this.unreadByFeed = new Map(Object.entries(lib.counts.unreadByFeed))
    this.starredCount = lib.counts.starred
  }

  /**
   * Loads the list for the current selection. In the Unread view, listed articles that
   * are no longer unread stay if `keep` says so, so the list doesn't jump while you read.
   */
  async #loadList(keep: (a: Article) => boolean = () => false) {
    const request = ++this.#listRequest
    const selection = $state.snapshot(this.selection)
    const unreadView = this.#isUnreadView
    await this.#writes
    let list = await api.articles(selection)
    if (request !== this.#listRequest) return

    if (unreadView) {
      const ids = new Set(list.map((a) => a.id))
      const kept = this.visible.filter((a) => !ids.has(a.id) && keep(a))
      if (kept.length > 0) {
        list = [...list, ...kept.map((a) => ({ ...a, read: this.read.has(a.id) }))]
        list.sort((a, b) => b.published - a.published)
      }
    }

    this.read.clear()
    this.starred.clear()
    for (const a of list) {
      if (a.read) this.read.add(a.id)
      if (a.starred) this.starred.add(a.id)
    }
    this.visible = list
  }
}

export const store = new Store()
