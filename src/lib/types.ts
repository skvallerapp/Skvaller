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
  /** The feed's address and its web site's, if known. */
  url: string | null
  siteUrl: string | null
  /** The local account's feeds: why the last fetch failed. */
  fetchError: string | null
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

/** When an account syncs on its own: manually, once on start, or on start and then
 * every so many minutes. */
export type SyncSchedule = 'manual' | 'onStart' | number

export type AccountKind =
  /** A sync service. `server` is the Google Reader API URL. */
  | { kind: 'greader'; server: string; username: string }
  /** Feeds fetched on this device. */
  | { kind: 'local' }

export type Account = AccountKind & {
  id: string
  /** The name the UI shows. The user can change it. */
  description: string
  sync: SyncSchedule
  /** How long read, unstarred articles stay in the cache. */
  keepReadDays: number
  /** Unread articles in its cache. */
  unread: number
}

export interface SyncStatus {
  /** The account it is about. */
  accountId: string
  syncing: boolean
  /** Unix time in milliseconds. */
  lastSync: number | null
  /** Why the last sync failed. */
  error: string | null
  /** The server rejected the token: the user has to sign in again. */
  unauthorized: boolean
}

export interface Session {
  accounts: Account[]
  /** The account the main window shows. `null` only without accounts. */
  activeId: string | null
  /** The active account can sync. False when its token was rejected: the sign-in screen
   * shows, filled in for it. */
  signedIn: boolean
  /** The active account's. */
  status: SyncStatus
  settings: Settings
}

/** What `updateAccount` changes. */
export type AccountChange = Partial<Pick<Account, 'description' | 'sync' | 'keepReadDays'>>

/** Settings for every account, from config.json, changed in the Settings window. */
export interface Settings {
  /** Ask before marking every article in a view as read. */
  confirmMarkAllRead: boolean
  theme: Theme
  /** Off stops the UI's animations. The system's reduced motion stops them as well. */
  animations: boolean
  uiFont: Font
  /** The article's headline, with its date and source. */
  headlineFont: Font
  /** The article's body text. */
  articleFont: Font
  /** Shortcut keys that differ from the defaults. `null` means no key. */
  shortcuts: Partial<Record<ShortcutAction, string | null>>
}

export type Theme = 'system' | 'light' | 'dark' | 'espresso'

export type Font = 'inter' | 'geist' | 'system' | 'source-serif' | 'literata' | 'serif'

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
  kind: 'unauthorized' | 'badUrl' | 'network' | 'noFeed' | 'other'
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
