// Font stacks for the `uiFont`, `headlineFont` and `articleFont` settings. The bundled fonts cover at
// least Latin, Cyrillic and Vietnamese. Every other script falls through to the system's
// fonts, which the browser picks per character.
import bundledCss from './fonts.css?inline'
import type { Font } from './types'

const system = "system-ui, 'Segoe UI', 'Noto Sans', sans-serif"
// ui-serif is New York on macOS; macOS doesn't expose it by name.
const serif = "ui-serif, Georgia, 'Noto Serif', serif"

export const fontStacks: Record<Font, string> = {
  inter: `'Inter Variable', ${system}`,
  geist: `'Geist Variable', ${system}`,
  system,
  'source-serif': `'Source Serif 4 Variable', ${serif}`,
  literata: `'Literata Variable', ${serif}`,
  serif,
}

/** The bundled fonts' @font-face rules, for documents that don't load app.css: the
 *  article frame. */
export const bundledFontFaces = bundledCss
