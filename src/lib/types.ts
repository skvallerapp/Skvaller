export interface Folder {
  id: string
  title: string
}

export interface Feed {
  id: string
  title: string
  folderId: string | null
}

export interface Article {
  id: string
  feedId: string
  title: string
  author: string
  url: string
  /** Unix time in milliseconds. */
  published: number
  summary: string
  /** Untrusted HTML from the feed. Render only in the sandboxed reader frame. */
  html: string
}

export type SmartView = 'unread' | 'starred' | 'all'

export type Selection =
  | { kind: 'smart'; view: SmartView }
  | { kind: 'folder'; id: string }
  | { kind: 'feed'; id: string }
