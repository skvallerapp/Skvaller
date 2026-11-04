// Typed wrappers for the Tauri commands in src-tauri/src/commands.rs.
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { openUrl } from '@tauri-apps/plugin-opener'
import { inTauri } from './platform'
import type { AccountChange, Article, Library, Selection, Session, Settings, SyncStatus } from './types'

// Outside Tauri (a plain browser during `bun run dev`), use mock data instead.
const mock = import.meta.env.DEV && !inTauri ? import('./mock') : null

const call = <T>(cmd: string, args?: Record<string, unknown>): Promise<T> =>
  mock ? mock.then((m) => m.invoke<T>(cmd, args)) : invoke<T>(cmd, args)

export const session = () => call<Session>('session')

/** Adds the account and makes it active, or signs in again to one already added. */
export const signIn = (server: string, username: string, password: string) =>
  call<Session>('sign_in', { server, username, password })

export const accounts = () => call<Session['accounts']>('accounts')

/** Shows another account in the main window. Every window gets `onAccountsChanged`. */
export const setActiveAccount = (id: string) => call<void>('set_active_account', { id })

export const updateAccount = (id: string, change: AccountChange) =>
  call<void>('update_account', { id, ...change })

/** Removes the account and its articles. Closes the Settings window when it was the last. */
export const removeAccount = (id: string) => call<void>('remove_account', { id })

/** Starts a sync of the active account. The outcome arrives through `onSyncStatus`. */
export const syncNow = () => call<void>('sync_now')

export const library = () => call<Library>('library')

export const articles = (selection: Selection) => call<Article[]>('articles', { selection })

export const articleHtml = (id: string) => call<string | null>('article_html', { id })

/** The article's Reader View HTML, extracted from its web page. Untrusted. Fetches the
 * page if it isn't cached, so it may take seconds or fail. A page given up on is fetched
 * again only with `retry`. */
export const fullText = (id: string, retry: boolean) => call<string>('full_text', { id, retry })

export const setFeedReaderView = (feedId: string, on: boolean) =>
  call<void>('set_feed_reader_view', { feedId, on })

export const setRead = (ids: string[], read: boolean) => call<void>('set_read', { ids, read })

export const setStarred = (ids: string[], starred: boolean) =>
  call<void>('set_starred', { ids, starred })

/** Opens an http(s) URL in the system browser, resolving a relative one against `base`.
 * Anything else is ignored. */
export function openInBrowser(url: string, base?: string) {
  let parsed: URL
  try {
    parsed = new URL(url.trim(), base || undefined)
  } catch {
    return
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return
  if (mock) window.open(parsed.href, '_blank', 'noopener')
  else openUrl(parsed.href).catch((e) => console.error('Opening a link failed', e))
}

export const setSettings = (settings: Settings) => call<void>('set_settings', { settings })

/** Opens the Settings window, at `pane` if given. */
export const openSettings = (pane?: string) => call<void>('open_settings', { pane })

/** The pane `openSettings` asked for, once. */
export const takeSettingsPane = () => call<string | null>('take_settings_pane')

function on<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
  return mock
    ? mock.then((m) => m.listen(event, handler as (payload: unknown) => void))
    : listen<T>(event, (e) => handler(e.payload))
}

export const onSyncStatus = (handler: (status: SyncStatus) => void) => on('sync-status', handler)

/** New settings, from any window. */
export const onSettingsChanged = (handler: (settings: Settings) => void) =>
  on('settings-changed', handler)

/** Accounts were added, changed or removed, or another one became active. */
export const onAccountsChanged = (handler: (session: Session) => void) => on('accounts-changed', handler)

/** The Settings window, open already, is asked to show a pane. */
export const onSettingsPane = (handler: (pane: string) => void) => on('settings-pane', handler)

/** The message of a command error, or of anything else thrown. */
export const errorMessage = (e: unknown): string =>
  (e as { message?: string } | null)?.message ?? String(e)

export const closeWindow = () => (mock ? window.close() : getCurrentWindow().close())
