<script lang="ts">
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import PanelLeft from '@lucide/svelte/icons/panel-left'
  import RotateCw from '@lucide/svelte/icons/rotate-cw'
  import Star from '@lucide/svelte/icons/star'
  import { tick, untrack } from 'svelte'
  import { fontStacks } from './fonts'
  import { formatTime } from './format.svelte'
  import { t } from './i18n.svelte'
  import { activePane, focus, layout, toggleSidebar } from './layout.svelte'
  import { thumbnailUrl } from './api'
  import { lineCount } from './lines.svelte'
  import { animate } from './motion'
  import { Overscroll } from './overscroll.svelte'
  import { dragRegion, macWindow } from './platform'
  import { store } from './store.svelte'
  import type { Article } from './types'
  import { viewInfo } from './views'
  import { dismissOutside } from './dismiss'

  // Row heights come from the data, never from measuring the DOM, so the list stays
  // virtualized and cheap at 5,000+ rows: the headline's lines are counted from canvas
  // measurements. Keep these in step with the styles below.
  /** A row's padding, above and below, and at the sides. */
  const PAD_Y = 11
  const PAD_X = 20
  const META_LINE = 16
  const TITLE_LINE = 19
  const TITLE_LINES = 3
  /** The space between a Small or Medium thumbnail and the text. */
  const GAP = 12
  /** Small and Medium thumbnail sizes. They have a 2 px margin above and below. */
  const THUMB = { small: 40, medium: 64 } as const
  const THUMB_MARGIN = 2
  /** The space above a Large thumbnail. */
  const LARGE_GAP = 8
  const OVERSCAN = 6

  let viewport: HTMLDivElement
  let scrollTop = $state(0)
  let height = $state(0)
  let width = $state(0)

  const items = $derived(store.visible)
  const thumbnails = $derived(store.settings.thumbnailSize)
  /** A Large thumbnail is 16:9 at the row's width, so its height follows the list's width. */
  const largeHeight = $derived(Math.round((Math.max(0, width - 2 * PAD_X) * 9) / 16))
  // Read headlines are lighter, but are measured bold, so marking an article read never
  // changes its row's height.
  const titleFont = $derived(`600 14.5px ${fontStacks[store.settings.uiFont]}`)
  const rowHeight = $derived.by(() => {
    const full = width - 2 * PAD_X
    const besideThumb = thumbnails === 'small' || thumbnails === 'medium'
    const thumb = besideThumb ? THUMB[thumbnails] + 2 * THUMB_MARGIN : 0
    const font = titleFont
    return (a: Article) => {
      const beside = besideThumb && a.thumbnail
      const lines = lineCount(a.title, font, beside ? full - THUMB[thumbnails] - GAP : full, TITLE_LINES)
      let content = META_LINE + lines * TITLE_LINE
      if (beside) content = Math.max(content, thumb)
      else if (thumbnails === 'large' && a.thumbnail) content += LARGE_GAP + largeHeight
      return 2 * PAD_Y + content
    }
  })
  /** Where each row starts, plus where the last one ends. */
  const offsets = $derived.by(() => {
    const o = new Float64Array(items.length + 1)
    for (let i = 0; i < items.length; i++) o[i + 1] = o[i] + rowHeight(items[i])
    return o
  })
  const selectedIndex = $derived(items.findIndex((a) => a.id === store.articleId))
  const first = $derived(Math.max(0, rowAt(offsets, scrollTop) - OVERSCAN))
  const last = $derived(Math.min(items.length, rowAt(offsets, scrollTop + height) + 1 + OVERSCAN))
  const rows = $derived(items.slice(first, last))

  /** The row at `y`: the number of rows that end at or above it. */
  function rowAt(o: Float64Array, y: number) {
    let lo = 0
    let hi = o.length - 1
    while (lo < hi) {
      const mid = (lo + hi) >> 1
      if (o[mid + 1] <= y) lo = mid + 1
      else hi = mid
    }
    return lo
  }

  const title = $derived.by(() => {
    const scope = store.scope
    if (scope.kind === 'all') return viewInfo(store.view).allLabel
    if (scope.kind === 'feed') return store.feed(scope.id)?.title ?? ''
    return store.folders.find((f) => f.id === scope.id)?.title ?? ''
  })

  const unread = $derived(store.unreadInView)
  const unreadLabel = $derived(unread === 0 ? t('list.noUnread') : t('list.unread', { count: unread }))

  // "Mark all as read" asks first, unless the confirmMarkAllRead setting is off.
  // The confirmation closes on Escape, Cancel, a click or focus elsewhere, leaving the
  // window, or when nothing is left to mark.
  let confirming = $state(false)
  let trigger: HTMLButtonElement
  let confirmButton = $state<HTMLButtonElement>()

  /** Moves on to the next feed or folder, if there is one; resolves once it's listed. */
  let { onmarkedall }: { onmarkedall: () => Promise<void> | undefined } = $props()

  let pane: HTMLDivElement

  // Pulling down at the top syncs, and the list waits there until no sync runs: one that
  // was already running makes the pull's own return at once.
  const overscroll = new Overscroll(async () => {
    await store.sync()
    await store.idle()
  })
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
    // It slides from where the list rests, not from a stretch.
    copy.style.transform = ''
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

  // Brings a newly opened article's row into view, such as when stepping through with
  // the keyboard. Only a change of article does this: a list reload or a resize must not
  // pull the list back while the user scrolls it.
  $effect(() => {
    const id = store.articleId
    const i = untrack(() => items.findIndex((a) => a.id === id))
    if (i === -1) return
    const [top, bottom] = untrack(() => [offsets[i], offsets[i + 1]])
    if (top < viewport.scrollTop) viewport.scrollTop = top
    else if (bottom > viewport.scrollTop + viewport.clientHeight)
      viewport.scrollTop = bottom - viewport.clientHeight
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
    scrolledPast = 0
    passed = []
    // Its list is laid out anew, not kept in place.
    laidOut = null
  })

  // When the rows change in the same list, as after a sync or when thumbnails arrive, or
  // their heights do, as when the thumbnail size changes, the list stays put: the selected
  // row keeps its place on screen if it's in view, otherwise the top row does, and a list
  // at the very top stays there so new articles show.
  let laidOut: Layout | null = null
  /** The list just moved to stay put: the highlight jumps with it instead of sliding. */
  let settling = $state(false)
  $effect(() => {
    const now = { items, offsets }
    const before = laidOut
    laidOut = now
    if (before && before.offsets !== now.offsets) untrack(() => keepInPlace(before, now))
    else if (!before) {
      // A new list starts at the top, even if the old one scrolled on while it loaded.
      untrack(() => {
        if (viewport.scrollTop !== 0) viewport.scrollTop = 0
        scrollTop = 0
        scrolledPast = 0
      })
    }
  })

  type Layout = { items: Article[]; offsets: Float64Array }

  function keepInPlace(before: Layout, now: Layout) {
    // Where the list is now, which a scroll may have changed before its event came. Unless
    // the browser just clamped a list that got shorter: that position is in the new rows.
    const live = Math.max(0, viewport.scrollTop)
    const clamped = live < scrollTop && live >= viewport.scrollHeight - viewport.clientHeight - 1
    const top = clamped ? scrollTop : live
    const [was, o] = [before.offsets, now.offsets]
    // That scroll passed rows of the old list, before its event could mark them.
    if (!clamped) pass(before.items, was, live)
    const selected = before.items.findIndex((a) => a.id === store.articleId)
    const shown = selected !== -1 && was[selected + 1] > top && was[selected] < top + height
    // At the very top, the list stays there, so new articles show.
    const next =
      top < 1 && !shown ? 0 : ((shown ? place(selected, true) : null) ?? place(rowAt(was, top), false))
    if (next !== null && Math.abs(next - live) >= 0.5) {
      settling = true
      viewport.scrollTop = next
      requestAnimationFrame(() => (settling = false))
    }
    scrollTop = Math.max(0, viewport.scrollTop)
    // The rows above changed, but none scrolled out.
    scrolledPast = rowAt(o, scrollTop)

    /** Where to scroll so row `anchor` keeps its place, or `null` if it's gone. The top
     * row stays as far into it as before, in proportion, so it stays the top row. */
    function place(anchor: number, exact: boolean): number | null {
      const id = before.items[anchor]?.id
      const at = now.items === before.items ? anchor : now.items.findIndex((a) => a.id === id)
      if (id === undefined || at === -1) return null
      const into = top - was[anchor]
      return exact ? o[at] + into : o[at] + (into / (was[anchor + 1] - was[anchor])) * (o[at + 1] - o[at])
    }
  }

  // With markReadOnScroll, rows that leave the top of the list while scrolling down are
  // marked read, once a frame. Scrolling up marks none.
  /** How many rows were above the top at the last scroll. */
  let scrolledPast = 0
  let passed: string[] = []
  let frame = 0

  function onScroll() {
    // overscroll-behavior keeps the list from going above the top, where it's supported.
    const top = Math.max(0, viewport.scrollTop)
    pass(items, offsets, top)
    scrollTop = top
  }

  /** Notes the rows of `list` (laid out at `o`) that the top of the list at `y` has
   * passed since the last scroll, and marks them read with markReadOnScroll. */
  function pass(list: Article[], o: Float64Array, y: number) {
    const past = rowAt(o, y)
    if (store.settings.markReadOnScroll && past > scrolledPast) {
      for (let i = scrolledPast; i < past; i++) passed.push(list[i].id)
      if (passed.length > 0 && frame === 0)
        frame = requestAnimationFrame(() => {
          frame = 0
          const ids = passed
          passed = []
          store.markRead(ids)
        })
    }
    scrolledPast = past
  }
</script>

<!-- Decorative: the headline says what the article is. -->
{#snippet thumbnail(article: Article)}
  <!-- Gone from the cache meanwhile, as after an account switch: no empty frame. -->
  <img
    class="thumb {thumbnails}"
    style:height={thumbnails === 'large' ? `${largeHeight}px` : null}
    src={thumbnailUrl(store.activeId ?? '', article.id)}
    alt=""
    decoding="async"
    onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = 'hidden')}
  />
{/snippet}

<svelte:window
  onkeydown={onWindowKeydown}
  onblur={() => (confirming = false)}
/>

<section class="list" class:active={activePane() === 'list'} aria-label={t('list.label')}>
  <div
    class="toolbar"
    class:window-buttons={macWindow && !layout.sidebarOpen}
    data-tauri-drag-region={dragRegion}
  >
    <button
      class="icon-button"
      aria-label={layout.sidebarOpen ? t('list.hideSidebar') : t('list.showSidebar')}
      aria-pressed={layout.sidebarOpen}
      onclick={toggleSidebar}
    >
      <PanelLeft size={18} strokeWidth={2} />
    </button>
    <div class="heading">
      <h1>{title}</h1>
      <p>{unreadLabel}</p>
    </div>
    <div class="mark-all" {@attach dismissOutside(() => (confirming = false))}>
      <button
        class="icon-button"
        bind:this={trigger}
        aria-label={t('list.markAll')}
        title={t('list.markAll')}
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
          <p id="mark-all-question">{t('list.markAllQuestion', { count: unread })}</p>
          <div class="actions">
            <button class="button" onclick={cancel}>{t('common.cancel')}</button>
            <button class="button primary" bind:this={confirmButton} onclick={confirmMarkAll}>
              {t('list.markAsRead')}
            </button>
          </div>
        </div>
      {/if}
    </div>
  </div>

  <div class="pane" bind:this={pane}>
    <!-- Decorative: the sidebar's sync button says the same. -->
    {#if overscroll.offset > 0 && (overscroll.fromTop || overscroll.refreshing)}
      <div
        class="refresh"
        class:armed={overscroll.armed}
        class:spinning={overscroll.refreshing}
        aria-hidden="true"
        style:height="{overscroll.hold}px"
        style:opacity={overscroll.armed ? 1 : overscroll.progress}
        style:transform="translateY({(overscroll.offset - overscroll.hold) / 2}px)"
      >
        <!-- A full turn when armed, so the spin starts where the pull left it. -->
        <RotateCw
          size={18}
          strokeWidth={2}
          style={overscroll.refreshing ? undefined : `transform: rotate(${overscroll.progress * 360}deg)`}
        />
      </div>
    {/if}
    <div
      class="viewport"
      tabindex="-1"
      bind:this={viewport}
      bind:clientHeight={height}
      bind:clientWidth={width}
      onscroll={onScroll}
      style:transform={overscroll.offset ? `translateY(${overscroll.offset}px)` : null}
      {@attach overscroll.attach}
    >
      {#if items.length === 0}
        <p class="empty">{viewInfo(store.view).empty}</p>
      {:else}
        <div class="spacer" style:height="{offsets[items.length]}px">
          <!-- The selection's background: one for the list, behind every row's text, so
               it can slide between rows. It goes when the selection does, so a new list's
               first selection appears in place. -->
          {#if selectedIndex !== -1}
            <div
              class="highlight"
              class:settling
              style:height="{offsets[selectedIndex + 1] - offsets[selectedIndex]}px"
              style:transform="translateY({offsets[selectedIndex]}px)"
            ></div>
          {/if}
          {#each rows as article, i (article.id)}
            <button
              class="row"
              class:selected={article.id === store.articleId}
              aria-current={article.id === store.articleId || undefined}
              class:unread={!store.read.has(article.id)}
              style:height="{offsets[first + i + 1] - offsets[first + i]}px"
              style:transform="translateY({offsets[first + i]}px)"
              onclick={() => {
                focus.pane = 'list'
                store.open(article.id)
              }}
            >
              {#if article.thumbnail && (thumbnails === 'small' || thumbnails === 'medium')}
                {@render thumbnail(article)}
              {/if}
              <span class="text">
                <span class="meta">
                  <span class="feed">{store.feed(article.feedId)?.title}</span>
                  {#if store.starred.has(article.id)}
                    <Star size={11} class="star" fill="currentColor" strokeWidth={0} />
                  {/if}
                  <time>{formatTime(article.published)}</time>
                </span>
                <span class="title">{article.title}</span>
                {#if article.thumbnail && thumbnails === 'large'}
                  {@render thumbnail(article)}
                {/if}
              </span>
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
    /* Overscroll stretches the list instead, alike on every platform. */
    overscroll-behavior: none;
    contain: strict;
  }

  /* Shows in the gap above a list pulled down, centered in it. Its height is the hold's. */
  .refresh {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-3);
    pointer-events: none;
  }

  .refresh.armed {
    color: var(--text-2);
  }

  .refresh.spinning :global(svg) {
    animation: spin 0.9s linear infinite;
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
    align-items: flex-start;
    gap: 12px;
    padding: 11px 20px;
    text-align: left;
    transition: color 120ms ease-out;
  }

  .text {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
  }

  /* Cropped to fill a fixed box, so rows never change height. The hairline keeps a light
     image from melting into a light list. */
  .thumb {
    flex: none;
    object-fit: cover;
    border-radius: 6px;
    outline: 1px solid var(--separator);
    outline-offset: -1px;
  }

  .thumb.small {
    width: 40px;
    height: 40px;
    margin-block: 2px;
  }

  .thumb.medium {
    width: 64px;
    height: 64px;
    margin-block: 2px;
  }

  /* Its height is set from the script, 16:9 at this width. */
  .thumb.large {
    width: 100%;
    margin-top: 8px;
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
    transition:
      transform 120ms ease-out,
      height 120ms ease-out;
  }

  .highlight.settling {
    transition: none;
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
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    line-height: 16px;
    font-weight: 400;
    text-transform: uppercase;
    color: var(--text-3);
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
    line-height: 19px;
  }

  .row:not(.unread) .title {
    font-weight: 500;
    color: var(--text-2);
  }

  .active .row.selected .meta,
  .active .row.selected .title {
    color: inherit;
  }

  .active .row.selected :global(.star) {
    color: inherit;
  }
</style>
