<script lang="ts">
  import Globe from '@lucide/svelte/icons/globe'
  import SquareText from '@lucide/svelte/icons/square-text'
  import Star from '@lucide/svelte/icons/star'
  import { bundledFontFaces, fontStacks } from './fonts'
  import { escapeHtml, formatFull } from './format'
  import { animate, motion } from './motion'
  import { dragRegion } from './platform'
  import { openInBrowser } from './api'
  import { store } from './store.svelte'
  import UnreadIcon from './UnreadIcon.svelte'
  import { palette } from './theme'
  import { untrack } from 'svelte'

  // Styles for the article frame. The frame has its own document, so app.css
  // doesn't reach it.
  const frameCss = $derived(`${bundledFontFaces}
    :root { color-scheme: light; --text: #2e2e2d; --text-2: #61605f; --text-3: #676665; --rule: rgb(0 0 0 / .1); --link: #9a5136; }
    :root[data-theme='dark'] { color-scheme: dark; --text: #e6e6e8; --text-2: #a0a0a6; --text-3: #97979d; --rule: rgb(255 255 255 / .1); --link: #8aa4c8; }
    :root[data-theme='espresso'] { color-scheme: dark; --text: #e0dedd; --text-2: #c1bdbc; --text-3: #9e928d; --rule: rgb(255 236 224 / .12); --link: #cfa080; }
    html { background: transparent; }
    body { margin: 0; padding: 36px 40px 64px; color: var(--text);
      font: 16px/1.65 ${fontStacks[store.settings.articleFont]}; -webkit-font-smoothing: antialiased; }
    article { max-width: 38em; margin: 0 auto; animation: enter 160ms ease-out; }
    @keyframes enter { from { opacity: 0; transform: translateY(4px); } }
    :root[data-motion='off'] article { animation: none; }
    header { margin-bottom: 32px; }
    header, h1, h2, h3, h4, h5, h6 { font-family: ${fontStacks[store.settings.headlineFont]}; }
    .date, .source { font-size: 12px; font-weight: 600; line-height: 1.3; text-transform: uppercase; color: var(--text-3); }
    h1 { margin: 2px 0 6px; font-size: 36px; line-height: 1.12; font-weight: 800; letter-spacing: -0.015em; }
    a { color: var(--link); text-decoration: none; }
    blockquote { margin: 1.4em 0; padding-left: 1em; border-left: 3px solid var(--rule); color: var(--text-2); }
    img, video { max-width: 100%; height: auto; }
    pre { overflow-x: auto; }
  `)

  const article = $derived(store.article)
  // Until Reader View content arrives, or if it fails, the feed's content shows.
  const showFullText = $derived(store.readerView && store.fullText !== null)
  const loadingFullText = $derived(store.readerView && store.fullTextState === 'loading')

  // Article HTML is untrusted. It goes into a sandboxed frame with no allow-scripts,
  // on top of the app CSP. allow-same-origin is there only so the frame can load the
  // bundled fonts; with scripts off, nothing in it can use that access. NEVER add
  // allow-scripts next to it: article scripts would then run as the app, with IPC.
  // Only our own header fields are escaped; the body is shown as the feed sent it.
  // The theme and motion are set on the frame's root, not written into the HTML, so a
  // change doesn't reload the article.
  const theme = $derived(palette(store.settings.theme))
  const frameMotion = $derived(motion(store.settings.animations))
  const srcdoc = $derived.by(() => {
    if (!article) return ''
    const feed = store.feed(article.feedId)?.title ?? ''
    return `<!doctype html><html data-theme="${untrack(() => theme)}" data-motion="${untrack(() => frameMotion)}"><head><meta charset="utf-8"><style>${frameCss}</style></head><body><article>
      <header>
        <div class="date">${escapeHtml(formatFull(article.published))}</div>
        <h1>${escapeHtml(article.title)}</h1>
        <div class="source">${escapeHtml(article.author)}<br>${escapeHtml(feed)}</div>
      </header>
      ${(showFullText ? store.fullText : store.html) ?? ''}
    </article></body></html>`
  })

  let { onkeydown }: { onkeydown: (e: KeyboardEvent) => void } = $props()
  let frame = $state<HTMLIFrameElement>()

  // The star pops when the shown article gets starred, not when a starred one opens.
  let pop = $state(false)
  let lastStar: { id?: string; starred: boolean } = { starred: false }
  $effect(() => {
    const id = article?.id
    const starred = id !== undefined && store.starred.has(id)
    pop = id === lastStar.id && starred && !lastStar.starred
    lastStar = { id, starred }
  })

  $effect(() => {
    const root = frame?.contentDocument?.documentElement
    if (!root) return
    root.dataset.theme = theme
    root.dataset.motion = frameMotion
  })

  // Links in the article open in the system browser, never in the frame. The frame is
  // same-origin, so the app can listen to its events while its own scripts stay off.
  // Relative links resolve against the article's URL, since the frame has no base.
  // Keys pressed while reading go to the app's shortcuts.
  function onFrameLoad() {
    const doc = frame?.contentDocument
    doc?.addEventListener('click', onLinkClick)
    doc?.addEventListener('auxclick', onLinkClick)
    doc?.addEventListener('keydown', onkeydown)
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

  /** Scrolls the article a page down (1) or up (-1). At its end, opens the next one. */
  export function readOn(direction: 1 | -1) {
    const doc = frame?.contentDocument
    if (!doc) {
      if (direction === 1 && !article) store.step(1)
      return
    }
    // A frame still loading has an empty document, which looks scrolled to its end.
    const page = doc.querySelector('article') && doc.scrollingElement
    if (!page) return
    if (direction === 1 && page.scrollTop + page.clientHeight >= page.scrollHeight - 1) {
      store.step(1)
      return
    }
    page.scrollBy({ top: direction * page.clientHeight * 0.9, behavior: animate() ? 'smooth' : 'auto' })
  }
</script>

<section class="reader" aria-label="Article">
  <div class="toolbar" data-tauri-drag-region={dragRegion}>
    {#if article}
      <div class="spacer">
        <!-- Announced, and shown until the next try or article. -->
        <span class="notice" role="status">
          {store.fullTextState === 'failed' ? "Reader View isn't available for this article" : ''}
        </span>
      </div>
      <button
        class="icon-button"
        aria-label={store.read.has(article.id) ? 'Mark as unread' : 'Mark as read'}
        onclick={() => store.toggleRead(article.id)}
      >
        <UnreadIcon size={18} read={store.read.has(article.id)} />
      </button>
      <button
        class="icon-button"
        class:starred={store.starred.has(article.id)}
        class:pop
        aria-label={store.starred.has(article.id) ? 'Remove star' : 'Star'}
        aria-pressed={store.starred.has(article.id)}
        onclick={() => store.toggleStarred(article.id)}
      >
        <Star size={18} strokeWidth={2} fill={store.starred.has(article.id) ? 'currentColor' : 'none'} />
      </button>
      <button
        class="icon-button"
        class:reader-view={store.readerView}
        class:loading={loadingFullText}
        aria-label="Reader View"
        aria-pressed={store.readerView}
        aria-busy={loadingFullText}
        title="Reader View"
        disabled={!article.url}
        onclick={() => store.toggleReaderView()}
      >
        <SquareText size={18} strokeWidth={2} />
      </button>
      <button
        class="icon-button"
        aria-label="Open in Browser"
        title="Open in Browser"
        disabled={!article.url}
        onclick={() => openInBrowser(article.url)}
      >
        <Globe size={18} strokeWidth={2} />
      </button>
    {/if}
  </div>

  {#if article}
    <!-- Create the frame only once the HTML is in, so it loads once. Chromium leaves
         a sandboxed frame blank when its srcdoc changes during the first load. -->
    <!-- Reader View that is fetched quickly, usually from the cache, shows without the
         feed's content flashing first. -->
    {#if store.html !== null && !(loadingFullText && store.awaitingFullText)}
      <!-- A new frame when Reader View content swaps in, for the same reason. -->
      {#key showFullText}
        <iframe title={article.title} sandbox="allow-same-origin" {srcdoc} bind:this={frame} onload={onFrameLoad}></iframe>
      {/key}
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
    min-width: 0;
  }

  .notice {
    display: block;
    padding-left: 12px;
    overflow: hidden;
    font-size: 12px;
    color: var(--text-3);
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .starred {
    color: var(--star);
  }

  .reader-view {
    color: var(--accent);
  }

  .loading {
    animation: pulse 1s ease-in-out infinite alternate;
  }

  @keyframes pulse {
    to {
      opacity: 0.35;
    }
  }

  /* Without the pulse, a dimmed icon shows the loading. */
  :global(:root[data-motion='off']) .loading {
    opacity: 0.5;
  }

  /* Starring an article, not opening a starred one. */
  .pop :global(svg) {
    animation: pop 220ms ease-out;
  }

  @keyframes pop {
    50% {
      transform: scale(1.25);
    }
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
