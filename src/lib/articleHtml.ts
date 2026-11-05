// Untrusted article HTML, readied for the sandboxed reader frame. DOMParser makes an
// inert document: nothing in it loads or runs while it's changed here.

/** `html` with media that waits to be asked for, and http resources asked for over
 * https, which the CSP requires. Most hosts that once served http now serve https too.
 * Doing this here, not with the CSP's upgrade-insecure-requests, keeps the upgrade out
 * of the app itself, which runs at http://tauri.localhost on Windows.
 * Links are readied with `readyLinks`, relative ones resolved against `base`, the
 * article's URL. */
export function readyArticle(html: string, base: string): string {
  const doc = new DOMParser().parseFromString(html, 'text/html')
  readyLinks(doc, base)
  // Without scripts, controls are the only way to play. Until then, nothing plays or
  // downloads.
  for (const media of doc.querySelectorAll('audio, video')) {
    media.removeAttribute('autoplay')
    media.setAttribute('preload', 'none')
    media.setAttribute('controls', '')
  }
  for (const el of doc.querySelectorAll('[src], [poster], [srcset]')) {
    for (const name of ['src', 'poster', 'srcset']) {
      const value = el.getAttribute(name)
      if (value) el.setAttribute(name, value.replace(/(^|,)(\s*)http:\/\//gi, '$1$2https://'))
    }
  }
  return doc.head.innerHTML + doc.body.innerHTML
}

/** Links that open in the system browser: each asks for a new window, which the app
 * refuses and opens in the browser instead (see `on_new_window` in main.rs). WebKit runs
 * no listeners in a frame without scripts, not even the app's, so a click handler can't.
 * A link within the article scrolls it. Its href is made `about:srcdoc#…`: a bare `#…`
 * resolves against the app's URL, and WebKit would load the app into the frame. */
function readyLinks(doc: Document, base: string) {
  for (const link of doc.querySelectorAll('a[href], area[href]')) {
    const href = link.getAttribute('href')!.trim()
    if (href.startsWith('#')) {
      link.setAttribute('href', `about:srcdoc${href}`)
      link.removeAttribute('target')
      continue
    }
    try {
      link.setAttribute('href', new URL(href, base || undefined).href)
      link.setAttribute('target', '_blank')
    } catch {
      // Left as is, it would resolve against the app's URL.
      link.removeAttribute('href')
    }
  }
}
