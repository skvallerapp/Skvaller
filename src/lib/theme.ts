// The `theme` setting. app.css and the article frame have a palette per theme, picked by
// `data-theme` on their root element.
import { MediaQuery } from 'svelte/reactivity'
import type { Theme } from './types'

const systemDark = new MediaQuery('(prefers-color-scheme: dark)')

/** The palette for a theme: System is Light or Dark, as the system is now. Reactive. */
export function palette(theme: Theme): Exclude<Theme, 'system'> {
  if (theme !== 'system') return theme
  return systemDark.current ? 'dark' : 'light'
}
