// Starting values, the same as Rust's defaults in src-tauri/src/config.rs.
import type { Settings, SyncStatus } from './types'

export const defaultSettings: Settings = {
  confirmMarkAllRead: true,
  theme: 'system',
  animations: true,
  uiFont: 'inter',
  headlineFont: 'inter',
  articleFont: 'literata',
  shortcuts: {},
}

export const idleStatus: SyncStatus = { accountId: '', syncing: false, lastSync: null, error: null, unauthorized: false }
