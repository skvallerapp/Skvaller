// Starting values, the same as Rust's defaults in src-tauri/src/config.rs.
import type { Settings, SyncStatus } from './types'

export const defaultSettings: Settings = {
  syncIntervalMinutes: 15,
  keepReadDays: 30,
  confirmMarkAllRead: true,
  uiFont: 'inter',
  articleFont: 'inter',
  shortcuts: {},
}

export const idleStatus: SyncStatus = { syncing: false, lastSync: null, error: null, unauthorized: false }
