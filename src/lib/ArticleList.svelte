<script lang="ts">
  import PanelLeft from '@lucide/svelte/icons/panel-left'
  import Star from '@lucide/svelte/icons/star'
  import { formatTime } from './format'
  import { layout, toggleSidebar } from './layout.svelte'
  import { store } from './store.svelte'

  // Fixed row height keeps the list virtualized and cheap at 5,000+ rows.
  const ROW_HEIGHT = 92
  const OVERSCAN = 6

  let viewport: HTMLDivElement
  let scrollTop = $state(0)
  let height = $state(0)

  const items = $derived(store.visible)
  const first = $derived(Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN))
  const last = $derived(Math.min(items.length, Math.ceil((scrollTop + height) / ROW_HEIGHT) + OVERSCAN))
  const rows = $derived(items.slice(first, last))

  const title = $derived.by(() => {
    const sel = store.selection
    if (sel.kind === 'smart') return { unread: 'Unread', starred: 'Starred', all: 'All Articles' }[sel.view]
    if (sel.kind === 'feed') return store.feed(sel.id)?.title ?? ''
    return store.folders.find((f) => f.id === sel.id)?.title ?? ''
  })

  // Back to the top whenever the selection in the sidebar changes.
  $effect(() => {
    void store.selection
    if (viewport) viewport.scrollTop = 0
    scrollTop = 0
  })
</script>

<section class="list" aria-label="Articles">
  <div class="toolbar">
    <button
      class="icon-button"
      aria-label={layout.sidebarOpen ? 'Hide sidebar' : 'Show sidebar'}
      aria-pressed={layout.sidebarOpen}
      onclick={toggleSidebar}
    >
      <PanelLeft size={17} strokeWidth={1.75} />
    </button>
    <h1>{title}</h1>
    <span class="total">{items.length}</span>
  </div>

  <div
    class="viewport"
    bind:this={viewport}
    bind:clientHeight={height}
    onscroll={() => (scrollTop = viewport.scrollTop)}
  >
    {#if items.length === 0}
      <p class="empty">No articles</p>
    {:else}
      <div class="spacer" style:height="{items.length * ROW_HEIGHT}px">
        {#each rows as article, i (article.id)}
          <button
            class="row"
            class:selected={article.id === store.articleId}
            class:unread={!store.read.has(article.id)}
            style:height="{ROW_HEIGHT}px"
            style:transform="translateY({(first + i) * ROW_HEIGHT}px)"
            onclick={() => store.open(article.id)}
          >
            <span class="meta">
              <span class="dot"></span>
              <span class="feed">{store.feed(article.feedId)?.title}</span>
              {#if store.starred.has(article.id)}
                <Star size={11} class="star" fill="currentColor" strokeWidth={0} />
              {/if}
              <time>{formatTime(article.published)}</time>
            </span>
            <span class="title">{article.title}</span>
            <span class="summary">{article.summary}</span>
          </button>
        {/each}
      </div>
    {/if}
  </div>
</section>

<style>
  .list {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--list-bg);
  }

  .toolbar {
    border-bottom: 1px solid var(--separator);
  }

  h1 {
    flex: 1;
    min-width: 0;
    margin: 0 0 0 4px;
    font-size: 13px;
    font-weight: 600;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .total {
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    color: var(--text-3);
  }

  .viewport {
    flex: 1;
    overflow-y: auto;
    contain: strict;
  }

  .spacer {
    position: relative;
  }

  .empty {
    margin: 40px 0;
    text-align: center;
    color: var(--text-3);
  }

  .row {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 11px 16px 0 22px;
    text-align: left;
    border-bottom: 1px solid var(--separator);
    overflow: hidden;
  }

  .row:hover {
    background: var(--hover);
  }

  .row.selected {
    background: var(--accent);
    color: var(--accent-text);
  }

  .meta {
    position: relative;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--text-3);
  }

  .dot {
    position: absolute;
    left: -13px;
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }

  .row.unread .dot {
    background: var(--unread-dot);
  }

  .feed {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .meta :global(.star) {
    color: var(--star);
  }

  time {
    font-variant-numeric: tabular-nums;
  }

  .title {
    font-size: 13.5px;
    font-weight: 600;
    line-height: 1.3;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .row:not(.unread) .title {
    font-weight: 500;
    color: var(--text-2);
  }

  .summary {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    font-size: 12.5px;
    line-height: 1.4;
    color: var(--text-2);
  }

  .row.selected .meta,
  .row.selected .title,
  .row.selected .summary,
  .row.selected .dot {
    color: inherit;
  }

  .row.selected.unread .dot {
    background: var(--accent-text);
  }

  .row.selected :global(.star) {
    color: inherit;
  }
</style>
