// The `theme` setting. app.css and the article frame have a palette per theme, picked by
// `data-theme` on their root element.
import { MediaQuery } from 'svelte/reactivity'
import { fontStacks } from './fonts'
import { setLanguage } from './i18n.svelte'
import { motion } from './motion'
import type { Settings, Theme } from './types'

const systemDark = new MediaQuery('(prefers-color-scheme: dark)')

/** The palette for a theme: System is Light or Dark, as the system is now. Reactive. */
export function palette(theme: Theme): Exclude<Theme, 'system'> {
  if (theme !== 'system') return theme
  return systemDark.current ? 'dark' : 'light'
}

/** Shows the window in the settings' language, theme, interface font and motion. Reactive. */
export function styleRoot(settings: Settings) {
  const root = document.documentElement
  root.style.fontFamily = fontStacks[settings.uiFont]
  root.dataset.theme = palette(settings.theme)
  root.dataset.motion = motion(settings.animations)
  setLanguage(settings.language)
}
