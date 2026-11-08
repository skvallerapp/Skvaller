import { mount } from 'svelte'
import './lib/fonts.css'
import './app.css'
import './settings.css'
import Settings from './Settings.svelte'
import * as api from './lib/api'
import { fontStacks } from './lib/fonts'
import { styleRoot } from './lib/theme'

// The window first paints in its settings' theme, language and font, instead of the
// browser's and then the settings' a moment later. A failure shows in the window.
api
  .session()
  .then(({ settings }) => {
    styleRoot(settings)
    return document.fonts.load(`1em ${fontStacks[settings.uiFont]}`)
  })
  .catch(() => {})
  .finally(() => mount(Settings, { target: document.getElementById('app')! }))
