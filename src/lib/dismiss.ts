import type { Attachment } from 'svelte/attachments'

/** Calls `close` when a pointer goes down outside the element, or focus moves to an
 * element outside it. Only focus moving somewhere closes it: WebKit doesn't focus a
 * clicked button, so a click inside leaves focus with nothing, and closing then would
 * remove the button before its click lands. */
export function dismissOutside(close: () => void): Attachment<HTMLElement> {
  return (node) => {
    const onPointerdown = (e: PointerEvent) => {
      if (!node.contains(e.target as Node)) close()
    }
    const onFocusout = (e: FocusEvent) => {
      const next = e.relatedTarget as Node | null
      if (next && !node.contains(next)) close()
    }
    window.addEventListener('pointerdown', onPointerdown)
    node.addEventListener('focusout', onFocusout)
    return () => {
      window.removeEventListener('pointerdown', onPointerdown)
      node.removeEventListener('focusout', onFocusout)
    }
  }
}
