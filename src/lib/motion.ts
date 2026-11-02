// The `animations` setting. When it's off, or the system asks for reduced motion,
// `data-motion="off"` on a root element stops every CSS animation and transition under
// it (app.css, and the article frame's own rule). Motion that CSS doesn't drive, such
// as Svelte transitions and smooth scrolling, asks `animate()`.
import { MediaQuery } from 'svelte/reactivity'

const reduced = new MediaQuery('(prefers-reduced-motion: reduce)')

/** The `data-motion` value for the setting and the system's preference. Reactive. */
export function motion(animations: boolean): 'on' | 'off' {
  return animations && !reduced.current ? 'on' : 'off'
}

/** Whether animations play in this window now. */
export const animate = () => document.documentElement.dataset.motion !== 'off'

/** A duration for a Svelte transition: 0 when animations are off. */
export const ms = (duration: number) => (animate() ? duration : 0)
