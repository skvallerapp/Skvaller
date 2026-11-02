import { mount } from 'svelte'
import './lib/fonts.css'
import './app.css'
import './settings.css'
import Settings from './Settings.svelte'

const app = mount(Settings, {
  target: document.getElementById('app')!,
})

export default app
