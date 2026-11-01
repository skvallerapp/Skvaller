/** macOS hides the title bar and draws its window buttons over the sidebar (see
 * tauri.conf.json), so the window has to be draggable from the toolbars instead. */
export const isMac = /Macintosh/.test(navigator.userAgent)

/** Value for `data-tauri-drag-region`: drag from the element and its non-interactive
 * children on macOS, nowhere elsewhere (the system title bar stays there). */
export const dragRegion = isMac ? 'deep' : undefined
