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
  /** The favicon as a data: URL. */
  icon: string | null
  /** Open its articles in Reader View. A local setting. */
  readerView: boolean
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
  starredByFeed: Record<string, number>
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
  settings: Settings
}

/** Settings from config.json, changed in the Settings window. */
export interface Settings {
  /** Minutes between automatic syncs. 0 means manually. */
  syncIntervalMinutes: number
  /** How long read, unstarred articles stay in the cache. */
  keepReadDays: number
  /** Ask before marking every article in a view as read. */
  confirmMarkAllRead: boolean
  uiFont: Font
  articleFont: Font
  /** Shortcut keys that differ from the defaults. `null` means no key. */
  shortcuts: Partial<Record<ShortcutAction, string | null>>
}

export type Font = 'inter' | 'system' | 'serif'

export type ShortcutAction =
  | 'next'
  | 'previous'
  | 'readOn'
  | 'readBack'
  | 'toggleRead'
  | 'toggleStar'
  | 'openInBrowser'
  | 'toggleReaderView'
  | 'sync'
  | 'markAllRead'

/** An error from a command. */
export interface CommandError {
  kind: 'unauthorized' | 'badUrl' | 'network' | 'other'
  message: string
}

/** Which articles to show, picked with the sidebar's button group. */
export type View = 'unread' | 'starred' | 'all'

/** Where to show them from, picked in the sidebar's tree. */
export type Scope = { kind: 'all' } | { kind: 'folder'; id: string } | { kind: 'feed'; id: string }

export interface Selection {
  view: View
  scope: Scope
}
