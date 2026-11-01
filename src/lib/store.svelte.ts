import { SvelteSet } from 'svelte/reactivity'
import * as api from './api'
import type { Account, Article, CommandError, Feed, Folder, Selection, SyncStatus } from './types'

/** All UI state. Everything comes from the local cache through `api`; nothing here
 * waits for the network. */
class Store {
  /** False until the session is known, so the app doesn't flash the sign-in screen. */
  ready = $state(false)
  signedIn = $state(false)
  account = $state<Account | null>(null)
  status = $state<SyncStatus>({ syncing: false, lastSync: null, error: null, unauthorized: false })

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
  // Reads wait for queued writes, so they never see the cache from before a change.
  #writes: Promise<unknown> = Promise.resolve()

  /** Feeds to show in the sidebar. Unsubscribed ones exist only for old starred articles. */
  readonly sidebarFeeds: Feed[] = $derived(this.feeds.filter((f) => f.subscribed))

  readonly unreadTotal: number = $derived(
    [...this.unreadByFeed.values()].reduce((sum, n) => sum + n, 0),
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
    void this.#loadList(false)
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

  toggleRead(id: string) {
    const read = !this.read.has(id)
    if (read) this.read.add(id)
    else this.read.delete(id)
    const feedId = this.visible.find((a) => a.id === id)?.feedId
    if (feedId) {
      const counts = new Map(this.unreadByFeed)
      counts.set(feedId, Math.max(0, (counts.get(feedId) ?? 0) + (read ? -1 : 1)))
      this.unreadByFeed = counts
    }
    this.#write(api.setRead(id, read))
  }

  toggleStarred(id: string) {
    const starred = !this.starred.has(id)
    if (starred) this.starred.add(id)
    else this.starred.delete(id)
    this.starredCount += starred ? 1 : -1
    this.#write(api.setStarred(id, starred))
  }

  /** Runs a local change, then reloads the counts from the cache to stay exact. */
  #write(change: Promise<void>) {
    this.#writes = this.#writes
      .then(() => change)
      .then(() => this.#loadLibrary())
      .catch((e) => console.error('Saving a change failed', e))
  }

  #onStatus(status: SyncStatus) {
    this.status = status
    if (status.unauthorized) this.signedIn = false
    else if (!status.syncing && !status.error) void this.#reload(true)
  }

  async #reload(keepRead = false) {
    await Promise.all([this.#loadLibrary(), this.#loadList(keepRead)])
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
   * Loads the list for the current selection. With `keepRead`, as after a sync, the
   * Unread view keeps the articles read since it was opened (FR-7a), so the list
   * doesn't jump while you read.
   */
  async #loadList(keepRead: boolean) {
    const request = ++this.#listRequest
    const selection = $state.snapshot(this.selection)
    await this.#writes
    let list = await api.articles(selection)
    if (request !== this.#listRequest) return

    const unreadView = selection.kind === 'smart' && selection.view === 'unread'
    if (keepRead && unreadView) {
      const ids = new Set(list.map((a) => a.id))
      const kept = this.visible.filter((a) => !ids.has(a.id) && this.read.has(a.id))
      if (kept.length > 0) {
        list = [...list, ...kept.map((a) => ({ ...a, read: true }))]
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
