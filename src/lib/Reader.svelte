<script lang="ts">
  import Circle from '@lucide/svelte/icons/circle'
  import CircleDot from '@lucide/svelte/icons/circle-dot'
  import Star from '@lucide/svelte/icons/star'
  import { escapeHtml, formatFull } from './format'
  import { store } from './store.svelte'

  // Styles for the article frame. The frame has its own document, so app.css
  // doesn't reach it.
  const frameCss = `
    :root { color-scheme: light dark; --text: #1c1c1e; --text-2: #6b6b70; --rule: rgb(0 0 0 / .1); --link: #2f6bd6; }
    @media (prefers-color-scheme: dark) {
      :root { --text: #e6e6e8; --text-2: #a0a0a6; --rule: rgb(255 255 255 / .1); --link: #6c9cf2; }
    }
    html { background: transparent; }
    body { margin: 0; padding: 36px 40px 64px; color: var(--text);
      font: 16px/1.65 system-ui, -apple-system, 'Segoe UI', sans-serif; -webkit-font-smoothing: antialiased; }
    article { max-width: 38em; margin: 0 auto; }
    header { margin-bottom: 28px; padding-bottom: 18px; border-bottom: 1px solid var(--rule); }
    .feed { font-size: 12px; font-weight: 600; color: var(--text-2); }
    h1 { margin: 6px 0 8px; font-size: 26px; line-height: 1.25; font-weight: 700; }
    .byline { font-size: 13px; color: var(--text-2); }
    a { color: var(--link); text-decoration: none; }
    blockquote { margin: 1.4em 0; padding-left: 1em; border-left: 3px solid var(--rule); color: var(--text-2); }
    img, video { max-width: 100%; height: auto; }
    pre { overflow-x: auto; }
  `

  const article = $derived(store.article)

  // Article HTML is untrusted. It goes into a sandboxed frame with no
  // allow-scripts, on top of the app CSP. Only our own header fields are escaped;
  // the body is shown as the feed sent it.
  const srcdoc = $derived.by(() => {
    if (!article) return ''
    const feed = store.feed(article.feedId)?.title ?? ''
    return `<!doctype html><html><head><meta charset="utf-8"><style>${frameCss}</style></head><body><article>
      <header>
        <div class="feed">${escapeHtml(feed)}</div>
        <h1>${escapeHtml(article.title)}</h1>
        <div class="byline">${escapeHtml(article.author)} · ${escapeHtml(formatFull(article.published))}</div>
      </header>
      ${article.html}
    </article></body></html>`
  })
</script>

<section class="reader" aria-label="Article">
  <div class="toolbar">
    {#if article}
      <div class="spacer"></div>
      <button
        class="icon-button"
        aria-label={store.read.has(article.id) ? 'Mark as unread' : 'Mark as read'}
        onclick={() => store.toggleRead(article.id)}
      >
        {#if store.read.has(article.id)}
          <Circle size={16} strokeWidth={1.75} />
        {:else}
          <CircleDot size={16} strokeWidth={1.75} />
        {/if}
      </button>
      <button
        class="icon-button"
        class:starred={store.starred.has(article.id)}
        aria-label={store.starred.has(article.id) ? 'Remove star' : 'Star'}
        aria-pressed={store.starred.has(article.id)}
        onclick={() => store.toggleStarred(article.id)}
      >
        <Star size={16} strokeWidth={1.75} fill={store.starred.has(article.id) ? 'currentColor' : 'none'} />
      </button>
    {/if}
  </div>

  {#if article}
    <iframe title={article.title} sandbox="" {srcdoc}></iframe>
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

  .starred:hover {
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
