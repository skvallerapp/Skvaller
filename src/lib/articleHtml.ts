// Untrusted article HTML, readied for the sandboxed reader frame. DOMParser makes an
// inert document: nothing in it loads or runs while it's changed here.

/** `html` with media that waits to be asked for, and http resources asked for over
 * https, which the CSP requires. Most hosts that once served http now serve https too.
 * Doing this here, not with the CSP's upgrade-insecure-requests, keeps the upgrade out
 * of the app itself, which runs at http://tauri.localhost on Windows. */
export function readyArticle(html: string): string {
  const doc = new DOMParser().parseFromString(html, 'text/html')
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
