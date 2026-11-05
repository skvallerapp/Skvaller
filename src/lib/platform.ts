/** False in a plain browser, as during `bun run dev` with mock data. */
export const inTauri = '__TAURI_INTERNALS__' in window

/** For key names and shortcuts, which follow the platform in a browser too. */
export const isMac = /Macintosh/.test(navigator.userAgent)

/** The app on macOS, which hides the title bar and draws its window buttons over the
 * sidebar (see tauri.conf.json). The layout makes room for the buttons, and the window
 * has to be draggable from the toolbars instead. A browser on a Mac has neither. */
export const macWindow = isMac && inTauri

/** Value for `data-tauri-drag-region`: drag from the element and its non-interactive
 * children on macOS, nowhere elsewhere (the system title bar stays there). */
export const dragRegion = macWindow ? 'deep' : undefined
