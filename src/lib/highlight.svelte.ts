import { untrack } from 'svelte'
import { animate } from './motion'

/** Slides the selected row's highlight from where the previous one was drawn. Each row
 *  holds a `.highlight` behind its text; `key` reads the selection. Call it while the
 *  component initializes, after any effect that scrolls the new row into view, so the
 *  row is measured where it ends up. */
export function slideHighlight(root: () => HTMLElement | undefined, key: () => unknown) {
  const highlight = () => root()?.querySelector<HTMLElement>('.selected .highlight')
  let from: number | null = null
  $effect.pre(() => {
    key()
    from = untrack(() => highlight()?.getBoundingClientRect().top ?? null)
  })
  $effect(() => {
    key()
    untrack(() => {
      const h = highlight()
      if (!h || from === null || !animate()) return
      const delta = from - h.getBoundingClientRect().top
      if (delta)
        h.animate([{ transform: `translateY(${delta}px)` }, { transform: 'none' }], {
          duration: 120,
          easing: 'ease-out',
        })
    })
  })
}
