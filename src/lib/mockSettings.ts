// The mock backend's settings, apart from mock.ts so api.ts can read them before the
// mock loads.
import { defaultSettings } from './defaults'
import type { Settings } from './types'

export const SETTINGS_KEY = 'skvaller.mock.settings'

export function mockSettings(): Settings {
  try {
    return { ...defaultSettings, ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? '{}') }
  } catch {
    return { ...defaultSettings }
  }
}
