// Typed wrappers for the Tauri commands in src-tauri/src/commands.rs.
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { Article, Library, Selection, Session, SyncStatus } from './types'

// Outside Tauri (a plain browser during `bun run dev`), use mock data instead.
const mock =
  import.meta.env.DEV && !('__TAURI_INTERNALS__' in window) ? import('./mock') : null

const call = <T>(cmd: string, args?: Record<string, unknown>): Promise<T> =>
  mock ? mock.then((m) => m.invoke<T>(cmd, args)) : invoke<T>(cmd, args)

export const session = () => call<Session>('session')

export const signIn = (server: string, username: string, password: string) =>
  call<Session>('sign_in', { server, username, password })

/** Starts a sync. The outcome arrives through `onSyncStatus`. */
export const syncNow = () => call<void>('sync_now')

export const library = () => call<Library>('library')

export const articles = (selection: Selection) => call<Article[]>('articles', { selection })

export const articleHtml = (id: string) => call<string | null>('article_html', { id })

export const setRead = (ids: string[], read: boolean) => call<void>('set_read', { ids, read })

export const setStarred = (ids: string[], starred: boolean) =>
  call<void>('set_starred', { ids, starred })

export const onSyncStatus = (handler: (status: SyncStatus) => void): Promise<UnlistenFn> =>
  mock
    ? mock.then((m) => m.onSyncStatus(handler))
    : listen<SyncStatus>('sync-status', (e) => handler(e.payload))
