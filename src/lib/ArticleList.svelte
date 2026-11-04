<script lang="ts">
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import PanelLeft from '@lucide/svelte/icons/panel-left'
  import Star from '@lucide/svelte/icons/star'
  import { tick, untrack } from 'svelte'
  import { formatTime } from './format'
  import { activePane, focus, layout, toggleSidebar } from './layout.svelte'
  import { animate } from './motion'
  import { dragRegion, macWindow } from './platform'
  import { store } from './store.svelte'
  import { viewInfo } from './views'

  // Fixed row height keeps the list virtualized and cheap at 5,000+ rows.
  // Room for a headline of three lines.
  const ROW_HEIGHT = 87
  const OVERSCAN = 6

  let viewport: HTMLDivElement
  let scrollTop = $state(0)
  let height = $state(0)

  const items = $derived(store.visible)
  const selectedIndex = $derived(items.findIndex((a) => a.id === store.articleId))
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

  /** Moves on to the next feed or folder, if there is one; resolves once it's listed. */
  let { onmarkedall }: { onmarkedall: () => Promise<void> | undefined } = $props()

  let pane: HTMLDivElement
  /** The copy of the old list, while it's on screen. */
  let ghost: HTMLElement | null = null

  function markAllAndMoveOn() {
    ghost?.remove()
    store.markAllRead()
    const next = onmarkedall()
    // The DOM still shows the old list until Svelte updates it, after this returns. A
    // copy of it slides up and away.
    if (next && animate()) void slideToNext(viewport.cloneNode(true) as HTMLElement, next)
  }

  /** As the next list slides in from below, the old one slides up out of sight. */
  async function slideToNext(copy: HTMLElement, next: Promise<void>) {
    const scope = store.scope
    ghost = copy
    copy.classList.add('ghost')
    copy.inert = true
    pane.append(copy)
    copy.scrollTop = viewport.scrollTop
    try {
      await next
      await tick()
    } catch {
      // The list stays as it was. Slide it in anyway, so the copy goes away.
    }
    // A newer mark-all replaced this one meanwhile.
    if (ghost !== copy) return
    // Something else was selected meanwhile: no next list to slide in.
    if (store.scope !== scope) {
      copy.remove()
      ghost = null
      return
    }
    const timing = { duration: 320, easing: 'cubic-bezier(0.3, 0, 0, 1)' }
    const leaving = copy.animate({ transform: ['none', 'translateY(-100%)'] }, timing)
    viewport.animate({ transform: ['translateY(100%)', 'none'] }, timing)
    leaving.onfinish = leaving.oncancel = () => {
      copy.remove()
      if (ghost === copy) ghost = null
    }
  }

  function onMarkAll() {
    if (store.settings.confirmMarkAllRead) confirming = !confirming
    else markAllAndMoveOn()
  }

  function cancel() {
    confirming = false
    trigger.focus()
  }

  function confirmMarkAll() {
    confirming = false
    markAllAndMoveOn()
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

  // Only focus moving to another element closes it here. WebKit doesn't focus a
  // clicked button, so a click in the confirmation leaves focus with nothing; closing
  // then would remove the button before its click lands. Clicks outside close it in
  // onWindowPointerdown.
  function onFocusout(e: FocusEvent) {
    const next = e.relatedTarget as Node | null
    if (next && !markAll.contains(next)) confirming = false
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
    // A row with keyboard focus (or, on Linux, a clicked one) passes it on, so Enter
    // and the focus ring stay on the open article.
    if (pane.contains(document.activeElement))
      void tick().then(() => pane.querySelector<HTMLElement>('.row.selected')?.focus({ preventScroll: true }))
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
    scrolledTo = 0
  })

  // With markReadOnScroll, rows that leave the top of the list while scrolling down are
  // marked read, once a frame. Scrolling up marks none.
  let scrolledTo = 0
  let passed: string[] = []
  let frame = 0

  function onScroll() {
    // Elastic overscroll can go above the top.
    const top = Math.max(0, viewport.scrollTop)
    if (store.settings.markReadOnScroll && top > scrolledTo) {
      const end = Math.min(items.length, Math.floor(top / ROW_HEIGHT))
      for (let i = Math.floor(scrolledTo / ROW_HEIGHT); i < end; i++) passed.push(items[i].id)
      if (passed.length > 0 && frame === 0)
        frame = requestAnimationFrame(() => {
          frame = 0
          const ids = passed
          passed = []
          store.markRead(ids)
        })
    }
    scrolledTo = top
    scrollTop = top
  }
</script>

<svelte:window
  onkeydown={onWindowKeydown}
  onpointerdown={onWindowPointerdown}
  onblur={() => (confirming = false)}
/>

<section class="list" class:active={activePane() === 'list'} aria-label="Articles">
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

  <div class="pane" bind:this={pane}>
    <div
      class="viewport"
      tabindex="-1"
      bind:this={viewport}
      bind:clientHeight={height}
      onscroll={onScroll}
    >
      {#if items.length === 0}
        <p class="empty">{viewInfo(store.view).empty}</p>
      {:else}
        <div class="spacer" style:height="{items.length * ROW_HEIGHT}px">
          <!-- The selection's background: one for the list, behind every row's text, so
               it can slide between rows. It goes when the selection does, so a new list's
               first selection appears in place. -->
          {#if selectedIndex !== -1}
            <div
              class="highlight"
              style:height="{ROW_HEIGHT}px"
              style:transform="translateY({selectedIndex * ROW_HEIGHT}px)"
            ></div>
          {/if}
          {#each rows as article, i (article.id)}
            <button
              class="row"
              class:selected={article.id === store.articleId}
              aria-current={article.id === store.articleId || undefined}
              class:unread={!store.read.has(article.id)}
              style:height="{ROW_HEIGHT}px"
              style:transform="translateY({(first + i) * ROW_HEIGHT}px)"
              onclick={() => {
                focus.pane = 'list'
                store.open(article.id)
              }}
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

  /* Clips the lists sliding in and out when all are marked read. */
  .pane {
    position: relative;
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .viewport {
    flex: 1;
    overflow-y: auto;
    contain: strict;
  }

  .pane > :global(.ghost) {
    position: absolute;
    inset: 0;
    background: var(--list-bg);
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
    padding: 11px 20px 0 22px;
    text-align: left;
    transition: color 120ms ease-out;
  }

  .row:hover {
    background: var(--hover);
  }

  .highlight {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    background: var(--accent-muted);
    transition: transform 120ms ease-out;
  }

  .active .highlight {
    background: var(--accent);
  }

  /* The highlight shows the focused selection; a ring would show ahead of its slide.
     Forced colors (High Contrast) override the color, so the ring shows there. */
  .row.selected:focus-visible {
    outline-color: transparent;
  }

  .meta,
  .title {
    transition: color 120ms ease-out;
  }

  /* The tint lowers the contrast of dimmer text, so it takes the full text color. */
  .row.selected .meta,
  .row.selected:not(.unread) .title {
    color: var(--text);
  }

  .active .row.selected {
    color: var(--accent-text);
  }

  .meta {
    position: relative;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    font-weight: 400;
    text-transform: uppercase;
    color: var(--text-3);
  }

  .dot {
    position: absolute;
    left: -13px;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    transition: background-color 120ms ease-out;
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
    font-size: 14.5px;
    font-weight: 600;
    line-height: 1.3;
  }

  .row:not(.unread) .title {
    font-weight: 500;
    color: var(--text-2);
  }

  .active .row.selected .meta,
  .active .row.selected .title,
  .active .row.selected .dot {
    color: inherit;
  }

  .active .row.selected.unread .dot {
    background: var(--accent-text);
  }

  .active .row.selected :global(.star) {
    color: inherit;
  }
</style>
