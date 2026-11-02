// Font stacks for the `uiFont` and `articleFont` settings. Inter ships with the app and
// covers Latin, Cyrillic, Greek and Vietnamese. Every other script falls through to the
// system's fonts, which the browser picks per character.
import interCss from '@fontsource-variable/inter/index.css?inline'
import interItalicCss from '@fontsource-variable/inter/wght-italic.css?inline'
import type { Font } from './types'

const system = "system-ui, 'Segoe UI', 'Noto Sans', sans-serif"

export const fontStacks: Record<Font, string> = {
  inter: `'Inter Variable', ${system}`,
  system,
  // ui-serif is New York on macOS; macOS doesn't expose it by name.
  serif: "ui-serif, Georgia, 'Noto Serif', serif",
}

/** Inter's @font-face rules, for documents that don't load app.css: the article frame. */
export const interFontFaces = interCss + interItalicCss
