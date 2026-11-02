<script lang="ts">
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import PanelLeft from '@lucide/svelte/icons/panel-left'
  import Star from '@lucide/svelte/icons/star'
  import { untrack } from 'svelte'
  import { formatTime } from './format'
  import { layout, toggleSidebar } from './layout.svelte'
  import { dragRegion, macWindow } from './platform'
  import { store } from './store.svelte'
  import { viewInfo } from './views'

  // Fixed row height keeps the list virtualized and cheap at 5,000+ rows.
  // Room for a headline of three lines.
  const ROW_HEIGHT = 102
  const OVERSCAN = 6

  let viewport: HTMLDivElement
  let scrollTop = $state(0)
  let height = $state(0)

  const items = $derived(store.visible)
  const first = $derived(Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN))
  const last = $derived(Math.min(items.length, Math.ceil((scrollTop + height) / ROW_HEIGHT) + OVERSCAN))
  const rows = $derived(items.slice(first, last))

  const title = $derived.by(() => {
    const scope = store.scope
    if (scope.kind === 'all') return viewInfo(store.view).allLabel
    if (scope.kind === 'feed') return store.feed(scope.id)?.title ?? ''
    return store.folders.find((f) => f.id === scope.id)?.title ?? ''
  })

  const unread = $derived(store.unreadInView)
  const unreadLabel = $derived(
    unread === 0
      ? 'No Unread Items'
      : `${unread.toLocaleString()} Unread ${unread === 1 ? 'Item' : 'Items'}`,
  )

  // "Mark all as read" asks first, unless the confirmMarkAllRead setting is off.
  // The confirmation closes on Escape, Cancel, a click or focus elsewhere, leaving the
  // window, or when nothing is left to mark.
  let confirming = $state(false)
  let markAll: HTMLDivElement
  let trigger: HTMLButtonElement
  let confirmButton = $state<HTMLButtonElement>()

  function onMarkAll() {
    if (store.settings.confirmMarkAllRead) confirming = !confirming
    else store.markAllRead()
  }

  function cancel() {
    confirming = false
    trigger.focus()
  }

  function confirmMarkAll() {
    confirming = false
    store.markAllRead()
    // The trigger is disabled now, so focus goes to the list.
    viewport.focus()
  }

  $effect(() => {
    if (confirming) confirmButton?.focus()
  })

  $effect(() => {
    if (unread === 0) confirming = false
  })

  function onWindowKeydown(e: KeyboardEvent) {
    if (confirming && e.key === 'Escape') cancel()
  }

  function onWindowPointerdown(e: PointerEvent) {
    if (confirming && !markAll.contains(e.target as Node)) confirming = false
  }

  function onFocusout(e: FocusEvent) {
    if (!markAll.contains(e.relatedTarget as Node | null)) confirming = false
  }

  // Brings a newly opened article's row into view, such as when stepping through with
  // the keyboard. Only a change of article does this: a list reload or a resize must not
  // pull the list back while the user scrolls it.
  $effect(() => {
    const id = store.articleId
    const i = untrack(() => items.findIndex((a) => a.id === id))
    if (i === -1) return
    const top = i * ROW_HEIGHT
    if (top < viewport.scrollTop) viewport.scrollTop = top
    else if (top + ROW_HEIGHT > viewport.scrollTop + viewport.clientHeight)
      viewport.scrollTop = top + ROW_HEIGHT - viewport.clientHeight
  })

  /** Marks all as read, asking first when the setting says so. */
  export function markAllRead() {
    if (unread > 0) onMarkAll()
  }

  // A new selection starts at the top, with no confirmation open.
  $effect(() => {
    void store.view
    void store.scope
    confirming = false
    if (viewport) viewport.scrollTop = 0
    scrollTop = 0
  })
</script>

<svelte:window
  onkeydown={onWindowKeydown}
  onpointerdown={onWindowPointerdown}
  onblur={() => (confirming = false)}
/>

<section class="list" aria-label="Articles">
  <div
    class="toolbar"
    class:window-buttons={macWindow && !layout.sidebarOpen}
    data-tauri-drag-region={dragRegion}
  >
    <button
      class="icon-button"
      aria-label={layout.sidebarOpen ? 'Hide sidebar' : 'Show sidebar'}
      aria-pressed={layout.sidebarOpen}
      onclick={toggleSidebar}
    >
      <PanelLeft size={18} strokeWidth={2} />
    </button>
    <div class="heading">
      <h1>{title}</h1>
      <p>{unreadLabel}</p>
    </div>
    <div class="mark-all" bind:this={markAll} onfocusout={onFocusout}>
      <button
        class="icon-button"
        bind:this={trigger}
        aria-label="Mark All as Read"
        title="Mark All as Read"
        aria-haspopup={store.settings.confirmMarkAllRead ? 'dialog' : undefined}
        aria-expanded={store.settings.confirmMarkAllRead ? confirming : undefined}
        disabled={unread === 0}
        onclick={onMarkAll}
      >
        <CircleCheck size={18} strokeWidth={2} />
      </button>
      {#if confirming}
        <div
          class="confirm"
          role="dialog"
          aria-labelledby="mark-all-question"
          data-tauri-drag-region="false"
        >
          <p id="mark-all-question">
            Mark {unread.toLocaleString()} {unread === 1 ? 'article' : 'articles'} as read?
          </p>
          <div class="actions">
            <button class="button" onclick={cancel}>Cancel</button>
            <button class="button primary" bind:this={confirmButton} onclick={confirmMarkAll}>
              Mark as Read
            </button>
          </div>
        </div>
      {/if}
    </div>
  </div>

  <div
    class="viewport"
    tabindex="-1"
    bind:this={viewport}
    bind:clientHeight={height}
    onscroll={() => (scrollTop = viewport.scrollTop)}
  >
    {#if items.length === 0}
      <p class="empty">{viewInfo(store.view).empty}</p>
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

  /* Room for the macOS window buttons while the sidebar is hidden. */
  .toolbar.window-buttons {
    padding-left: 84px;
  }

  .toolbar {
    /* Same timing as the sidebar, so the toggle never slides under the window buttons. */
    transition: padding-left 180ms ease;
    border-bottom: 1px solid var(--separator);
  }

  .heading {
    flex: 1;
    min-width: 0;
    margin-left: 6px;
  }

  h1,
  .heading p {
    margin: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  h1 {
    font-size: 14px;
    font-weight: 700;
    line-height: 1.2;
  }

  .heading p {
    font-size: 12px;
    line-height: 1.3;
    color: var(--text-3);
  }

  .mark-all {
    position: relative;
  }

  .icon-button:disabled {
    opacity: 0.35;
  }

  .confirm {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 10;
    width: 240px;
    padding: 14px;
    border: 1px solid var(--separator);
    border-radius: 10px;
    background: var(--reader-bg);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.18);
    animation: appear 140ms ease-out;
    transform-origin: top right;
  }

  .confirm p {
    margin: 0 0 12px;
    font-weight: 600;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
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
    font-size: 11.5px;
    font-weight: 400;
    letter-spacing: 0.03em;
    text-transform: uppercase;
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

  .title {
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
    font-size: 15.5px;
    font-weight: 600;
    line-height: 1.3;
  }

  .row:not(.unread) .title {
    font-weight: 500;
    color: var(--text-2);
  }

  .row.selected .meta,
  .row.selected .title,
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
