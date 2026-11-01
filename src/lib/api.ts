// Typed wrappers for the Tauri commands in src-tauri/src/commands.rs.
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { Article, Library, Selection, Session, SyncStatus } from './types'

export const session = () => invoke<Session>('session')

export const signIn = (server: string, username: string, password: string) =>
  invoke<Session>('sign_in', { server, username, password })

/** Starts a sync. The outcome arrives through `onSyncStatus`. */
export const syncNow = () => invoke<void>('sync_now')

export const library = () => invoke<Library>('library')

export const articles = (selection: Selection) => invoke<Article[]>('articles', { selection })

export const articleHtml = (id: string) => invoke<string | null>('article_html', { id })

export const setRead = (id: string, read: boolean) => invoke<void>('set_read', { id, read })

export const setStarred = (id: string, starred: boolean) =>
  invoke<void>('set_starred', { id, starred })

export const onSyncStatus = (handler: (status: SyncStatus) => void): Promise<UnlistenFn> =>
  listen<SyncStatus>('sync-status', (e) => handler(e.payload))
