export interface Folder {
  id: string
  title: string
}

export interface Feed {
  id: string
  title: string
  folderId: string | null
  /** False for a feed kept only for the starred articles of an old subscription. */
  subscribed: boolean
}

/** An article in the list. The HTML is loaded separately, when the article opens. */
export interface Article {
  /** Decimal item ID. A string: it can exceed what a JavaScript number holds. */
  id: string
  feedId: string
  title: string
  author: string
  url: string
  /** Unix time in milliseconds. */
  published: number
  /** Plain-text preview. */
  summary: string
  read: boolean
  starred: boolean
}

export interface Counts {
  unreadByFeed: Record<string, number>
  starred: number
}

export interface Library {
  folders: Folder[]
  feeds: Feed[]
  counts: Counts
}

export interface Account {
  /** The Google Reader API URL. */
  server: string
  username: string
}

export interface SyncStatus {
  syncing: boolean
  /** Unix time in milliseconds. */
  lastSync: number | null
  /** Why the last sync failed. */
  error: string | null
  /** The server rejected the token: the user has to sign in again. */
  unauthorized: boolean
}

export interface Session {
  /** The saved account, also when signed out, to fill in the sign-in form. */
  account: Account | null
  signedIn: boolean
  status: SyncStatus
}

/** An error from a command. */
export interface CommandError {
  kind: 'unauthorized' | 'badUrl' | 'network' | 'other'
  message: string
}

export type SmartView = 'unread' | 'starred' | 'all'

export type Selection =
  | { kind: 'smart'; view: SmartView }
  | { kind: 'folder'; id: string }
  | { kind: 'feed'; id: string }
