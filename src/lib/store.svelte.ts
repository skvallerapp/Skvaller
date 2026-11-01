import { SvelteSet } from 'svelte/reactivity'
import { articles, feeds, folders, initialRead, initialStarred } from './mock'
import type { Article, Selection } from './types'

const feedById = new Map(feeds.map((f) => [f.id, f]))

class Store {
  readonly folders = folders
  readonly feeds = feeds
  readonly read = new SvelteSet<string>(initialRead)
  readonly starred = new SvelteSet<string>(initialStarred)

  selection = $state<Selection>({ kind: 'smart', view: 'unread' })
  articleId = $state<string | null>(null)

  // The Unread view filters on the read state from when it was opened, so an
  // article doesn't vanish from the list the moment you read it.
  #readAtSelect = $state.raw(new Set(initialRead))

  readonly visible: Article[] = $derived.by(() => {
    const sel = this.selection
    switch (sel.kind) {
      case 'smart':
        if (sel.view === 'all') return articles
        if (sel.view === 'starred') return articles.filter((a) => this.starred.has(a.id))
        return articles.filter((a) => !this.#readAtSelect.has(a.id))
      case 'feed':
        return articles.filter((a) => a.feedId === sel.id)
      case 'folder':
        return articles.filter((a) => feedById.get(a.feedId)?.folderId === sel.id)
    }
  })

  readonly unreadByFeed: Map<string, number> = $derived.by(() => {
    const counts = new Map<string, number>()
    for (const a of articles) {
      if (!this.read.has(a.id)) counts.set(a.feedId, (counts.get(a.feedId) ?? 0) + 1)
    }
    return counts
  })

  readonly unreadTotal: number = $derived(
    [...this.unreadByFeed.values()].reduce((sum, n) => sum + n, 0),
  )

  readonly article: Article | null = $derived(
    articles.find((a) => a.id === this.articleId) ?? null,
  )

  feed(id: string) {
    return feedById.get(id)
  }

  folderUnread(folderId: string): number {
    let n = 0
    for (const f of feeds) if (f.folderId === folderId) n += this.unreadByFeed.get(f.id) ?? 0
    return n
  }

  select(selection: Selection) {
    this.selection = selection
    this.articleId = null
    this.#readAtSelect = new Set(this.read)
  }

  open(id: string) {
    this.articleId = id
    this.read.add(id)
  }

  toggleRead(id: string) {
    if (!this.read.delete(id)) this.read.add(id)
  }

  toggleStarred(id: string) {
    if (!this.starred.delete(id)) this.starred.add(id)
  }
}

export const store = new Store()
