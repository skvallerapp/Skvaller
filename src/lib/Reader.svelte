<script lang="ts">
  import Circle from '@lucide/svelte/icons/circle'
  import CircleDot from '@lucide/svelte/icons/circle-dot'
  import Compass from '@lucide/svelte/icons/compass'
  import Star from '@lucide/svelte/icons/star'
  import { fontStacks, interFontFaces } from './fonts'
  import { escapeHtml, formatFull } from './format'
  import { dragRegion } from './platform'
  import { openInBrowser } from './api'
  import { store } from './store.svelte'

  // Styles for the article frame. The frame has its own document, so app.css
  // doesn't reach it.
  const frameCss = $derived(`${interFontFaces}
    :root { color-scheme: light dark; --text: #1c1c1e; --text-2: #6b6b70; --text-3: #8e8e93; --rule: rgb(0 0 0 / .1); --link: #2f6bd6; }
    @media (prefers-color-scheme: dark) {
      :root { --text: #e6e6e8; --text-2: #a0a0a6; --text-3: #8a8580; --rule: rgb(255 255 255 / .1); --link: #6c9cf2; }
    }
    html { background: transparent; }
    body { margin: 0; padding: 36px 40px 64px; color: var(--text);
      font: 16px/1.65 ${fontStacks[store.settings.articleFont]}; -webkit-font-smoothing: antialiased; }
    article { max-width: 38em; margin: 0 auto; }
    header { margin-bottom: 32px; }
    .date, .source { font-size: 12px; font-weight: 600; line-height: 1.3; text-transform: uppercase; color: var(--text-3); }
    h1 { margin: 2px 0 6px; font-size: 36px; line-height: 1.12; font-weight: 800; letter-spacing: -0.015em; }
    a { color: var(--link); text-decoration: none; }
    blockquote { margin: 1.4em 0; padding-left: 1em; border-left: 3px solid var(--rule); color: var(--text-2); }
    img, video { max-width: 100%; height: auto; }
    pre { overflow-x: auto; }
  `)

  const article = $derived(store.article)

  // Article HTML is untrusted. It goes into a sandboxed frame with no allow-scripts,
  // on top of the app CSP. allow-same-origin is there only so the frame can load the
  // bundled fonts; with scripts off, nothing in it can use that access. NEVER add
  // allow-scripts next to it: article scripts would then run as the app, with IPC.
  // Only our own header fields are escaped; the body is shown as the feed sent it.
  const srcdoc = $derived.by(() => {
    if (!article) return ''
    const feed = store.feed(article.feedId)?.title ?? ''
    return `<!doctype html><html><head><meta charset="utf-8"><style>${frameCss}</style></head><body><article>
      <header>
        <div class="date">${escapeHtml(formatFull(article.published))}</div>
        <h1>${escapeHtml(article.title)}</h1>
        <div class="source">${escapeHtml(article.author)}<br>${escapeHtml(feed)}</div>
      </header>
      ${store.html ?? ''}
    </article></body></html>`
  })

  let frame = $state<HTMLIFrameElement>()

  // Links in the article open in the system browser, never in the frame. The frame is
  // same-origin, so the app can listen to its events while its own scripts stay off.
  // Relative links resolve against the article's URL, since the frame has no base.
  function onFrameLoad() {
    const doc = frame?.contentDocument
    doc?.addEventListener('click', onLinkClick)
    doc?.addEventListener('auxclick', onLinkClick)
  }

  function onLinkClick(e: MouseEvent) {
    if (e.button > 1) return
    const link = (e.target as Element).closest?.('a, area')
    if (!link) return
    const href = (
      link.getAttribute('href') ?? link.getAttributeNS('http://www.w3.org/1999/xlink', 'href') ?? ''
    ).trim()
    // A link within the article scrolls it.
    if (href.startsWith('#')) return
    e.preventDefault()
    if (href) openInBrowser(href, article?.url)
  }
</script>

<section class="reader" aria-label="Article">
  <div class="toolbar" data-tauri-drag-region={dragRegion}>
    {#if article}
      <div class="spacer"></div>
      <button
        class="icon-button"
        aria-label={store.read.has(article.id) ? 'Mark as unread' : 'Mark as read'}
        onclick={() => store.toggleRead(article.id)}
      >
        {#if store.read.has(article.id)}
          <Circle size={18} strokeWidth={2} />
        {:else}
          <CircleDot size={18} strokeWidth={2} />
        {/if}
      </button>
      <button
        class="icon-button"
        class:starred={store.starred.has(article.id)}
        aria-label={store.starred.has(article.id) ? 'Remove star' : 'Star'}
        aria-pressed={store.starred.has(article.id)}
        onclick={() => store.toggleStarred(article.id)}
      >
        <Star size={18} strokeWidth={2} fill={store.starred.has(article.id) ? 'currentColor' : 'none'} />
      </button>
      <button
        class="icon-button"
        aria-label="Open in Browser"
        title="Open in Browser"
        disabled={!article.url}
        onclick={() => openInBrowser(article.url)}
      >
        <Compass size={18} strokeWidth={2} />
      </button>
    {/if}
  </div>

  {#if article}
    <!-- Create the frame only once the HTML is in, so it loads once. Chromium leaves
         a sandboxed frame blank when its srcdoc changes during the first load. -->
    {#if store.html !== null}
      <iframe title={article.title} sandbox="allow-same-origin" {srcdoc} bind:this={frame} onload={onFrameLoad}></iframe>
    {/if}
  {:else}
    <p class="empty">No article selected</p>
  {/if}
</section>

<style>
  .reader {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--reader-bg);
  }

  .spacer {
    flex: 1;
  }

  .starred {
    color: var(--star);
  }

  iframe {
    flex: 1;
    width: 100%;
    border: 0;
  }

  .empty {
    margin: auto;
    padding-bottom: var(--toolbar-height);
    color: var(--text-3);
  }
</style>
