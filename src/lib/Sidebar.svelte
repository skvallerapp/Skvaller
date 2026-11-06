<script lang="ts">
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import CircleAlert from '@lucide/svelte/icons/circle-alert'
  import RotateCw from '@lucide/svelte/icons/rotate-cw'
  import SettingsIcon from '@lucide/svelte/icons/settings'
  import Rss from '@lucide/svelte/icons/rss'
  import { CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu } from '@tauri-apps/api/menu'
  import { onDestroy, tick, untrack } from 'svelte'
  import { slide, type TransitionConfig } from 'svelte/transition'
  import { SvelteSet } from 'svelte/reactivity'
  import AccountSwitcher from './AccountSwitcher.svelte'
  import AddFeed from './AddFeed.svelte'
  import { formatSynced } from './format'
  import { confirm, openSettings } from './api'
  import { animate, ms } from './motion'
  import { activePane, focus } from './layout.svelte'
  import { dragRegion, inTauri, isMac, macWindow } from './platform'
  import { store } from './store.svelte'
  import type { Feed, Folder, Scope } from './types'
  import { slideHighlight } from './highlight.svelte'
  import { viewInfo, views } from './views'

  const current = $derived(viewInfo(store.view))

  const collapsed = new SvelteSet<string>()

  // The Unread and Starred views show only the feeds and folders with articles in them.
  // Feeds that had some when the view or selection was picked, or a sync finished, stay
  // until the next of those, so rows don't jump while you read (as FR-7a does for the
  // list). The selected feed or folder always shows.
  const filtering = $derived(store.view !== 'all')
  const pinned = $derived.by(() => {
    void store.view
    void store.scope
    void store.status.lastSync
    return untrack(() => new Set([...store.counts].filter(([, n]) => n > 0).map(([id]) => id)))
  })
  const shownFeeds = $derived(
    store.sidebarFeeds.filter(
      (f) =>
        !filtering ||
        (store.counts.get(f.id) ?? 0) > 0 ||
        pinned.has(f.id) ||
        isSelected({ kind: 'feed', id: f.id }),
    ),
  )
  const feedsByFolder = $derived.by(() => {
    const map = new Map<string | null, Feed[]>()
    for (const f of shownFeeds) {
      const list = map.get(f.folderId)
      if (list) list.push(f)
      else map.set(f.folderId, [f])
    }
    return map
  })
  const shownFolders = $derived(
    store.folders.filter(
      (f) => !filtering || feedsByFolder.has(f.id) || isSelected({ kind: 'folder', id: f.id }),
    ),
  )
  const looseFeeds = $derived(feedsByFolder.get(null) ?? [])
  const title = $derived(store.account?.description ?? '')
  let rebuilding = $state(false)
  async function rebuild() {
    rebuilding = true
    try {
      await store.rebuildCache()
    } catch (e) {
      console.error('Rebuilding the cache failed', e)
    } finally {
      rebuilding = false
    }
  }

  const status = $derived.by(() => {
    const s = store.status
    if (s.syncing) return 'Syncing…'
    if (s.error) return 'Sync failed'
    return s.lastSync ? formatSynced(s.lastSync) : 'Not synced yet'
  })

  /** SQLite's NOCASE order, which the cache sorts each list by: ASCII case folded, the
   * rest by code point, as UTF-8 bytes compare. */
  const nocase = (a: string, b: string) => {
    const points = (s: string) => Array.from(s, (c) => c.codePointAt(0)! + (c >= 'A' && c <= 'Z' ? 32 : 0))
    const [x, y] = [points(a), points(b)]
    for (let i = 0; i < Math.min(x.length, y.length); i++) if (x[i] !== y[i]) return x[i] - y[i]
    return x.length - y.length
  }

  // The folders and the feeds in no folder, in the order the setting asks: folders first,
  // or all A–Z together. Each list is A–Z already.
  type Entry = { kind: 'folder'; id: string; folder: Folder } | { kind: 'feed'; id: string; feed: Feed }
  const entries: Entry[] = $derived.by(() => {
    const folders = shownFolders.map((folder): Entry => ({ kind: 'folder', id: folder.id, folder }))
    const feeds = looseFeeds.map((feed): Entry => ({ kind: 'feed', id: feed.id, feed }))
    if (store.settings.sortSubscriptions !== 'alphabetical') return [...folders, ...feeds]
    const title = (e: Entry) => (e.kind === 'folder' ? e.folder.title : e.feed.title)
    return [...folders, ...feeds].sort((a, b) => nocase(title(a), title(b)))
  })

  // The rows below All, in sidebar order: each folder, then its feeds when open, and the
  // feeds in no folder.
  type Row = Exclude<Scope, { kind: 'all' }>
  const rows: Row[] = $derived(
    entries.flatMap((entry): Row[] =>
      entry.kind === 'feed'
        ? [{ kind: 'feed', id: entry.id }]
        : [
            { kind: 'folder', id: entry.id },
            ...(collapsed.has(entry.id) ? [] : (feedsByFolder.get(entry.id) ?? [])).map(
              (f): Row => ({ kind: 'feed', id: f.id }),
            ),
          ],
    ),
  )

  // Rows slide closed as they leave only right after a mark-all-read. Other changes, such
  // as switching views, add and remove many rows, and do so at once.
  // They fade too, so the row that was selected doesn't look selected next to the new one.
  let rowsLeaving = false
  function leave(node: Element): TransitionConfig {
    // Measuring a row isn't free, and a view switch removes many.
    if (!rowsLeaving || !animate()) return {}
    const closing = slide(node, { duration: 220 })
    return { ...closing, css: (t, u) => `${closing.css?.(t, u)}; opacity: ${t}` }
  }

  /** After the Unread view's feed or folder is marked read, selects the next row below
   * with unread articles, or All Unread when there is none. The marked row then hides.
   * Resolves once the new selection is listed. */
  export function selectNext(): Promise<void> | undefined {
    if (store.view !== 'unread') return
    const i = rows.findIndex(isSelected)
    if (i === -1) return
    const unread = (s: Row) =>
      ((s.kind === 'folder' ? store.folderCounts : store.counts).get(s.id) ?? 0) > 0
    rowsLeaving = true
    void tick().then(() => (rowsLeaving = false))
    return store.select(rows.slice(i + 1).find(unread) ?? { kind: 'all' })
  }

  let nav: HTMLElement

  // The selection scrolls into view, before its highlight slides there. Each row has its
  // own highlight, since rows move as folders open and close.
  $effect(() => {
    void store.scope
    untrack(() => nav.querySelector('.row.selected')?.scrollIntoView({ block: 'nearest' }))
  })
  slideHighlight(() => nav, () => store.scope)

  /** Selects the row above or below, as the arrow keys do while the sidebar is in focus. */
  export async function step(delta: 1 | -1) {
    const all: Scope[] = [{ kind: 'all' }, ...rows]
    let i = all.findIndex(isSelected)
    // A feed in a collapsed folder has no row: step from its folder's.
    const scope = store.scope
    if (i === -1 && scope.kind === 'feed') {
      const folderId = store.feed(scope.id)?.folderId
      i = all.findIndex((r) => r.kind === 'folder' && r.id === folderId)
    }
    const next = all[i + delta]
    if (!next) return
    // A row with keyboard focus (or, on Linux, a clicked one) passes it on, so Enter
    // and the focus ring stay on the selection.
    const hadFocus = nav.contains(document.activeElement)
    void store.select(next)
    await tick()
    if (hadFocus) nav.querySelector<HTMLElement>('.row.selected')?.focus({ preventScroll: true })
  }

  function isSelected(scope: Scope): boolean {
    const cur = store.scope
    if (cur.kind !== scope.kind) return false
    return cur.kind === 'all' || cur.id === (scope as typeof cur).id
  }

  let viewButtons = $state<HTMLButtonElement[]>([])
  const viewIndex = $derived(views.findIndex((v) => v.view === store.view))

  /** Arrow keys move between the views, as in a radio group. */
  function onViewsKeydown(e: KeyboardEvent) {
    const step = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 }[e.key]
    if (!step) return
    e.preventDefault()
    const i = (viewIndex + step + views.length) % views.length
    store.setView(views[i].view)
    viewButtons[i].focus()
  }

  function toggleFolder(id: string) {
    if (!collapsed.delete(id)) collapsed.add(id)
  }

  // One native menu for every feed, made on first use: menus are app resources that
  // stay until closed, and on Linux `popup` returns before the menu does. The local
  // account's menu lists the folders to move to, so it is made again when they change,
  // and the old one closed then, long after its popup.
  interface FeedMenu {
    menu: Menu
    readerView: CheckMenuItem
    /** "Move To" items, by folder ID; `null` is no folder. */
    moves: [string | null, CheckMenuItem][]
    /** Every resource made for the menu, the menu last: each one is closed. */
    resources: { close(): Promise<void> }[]
  }
  let feedMenu: Promise<FeedMenu> | null = null
  let feedMenuKey = ''
  // The feed or folder the menu was last opened for. Its actions act on this one.
  let menuFeedId = ''
  let menuFolderId = ''
  let menuRequest = 0

  function makeFeedMenu(): Promise<FeedMenu> {
    const key = store.isLocal ? JSON.stringify(store.folders) : ''
    if (feedMenu && key === feedMenuKey) return feedMenu
    const old = feedMenu
    feedMenuKey = key
    // As of now: the account may change while the menu is made.
    const local = store.isLocal
    const folders = store.folders
    const menu = (async (): Promise<FeedMenu> => {
      void old?.then(closeAll).catch(() => {})
      const readerView = await CheckMenuItem.new({
        text: 'Always Use Reader View',
        action: () => {
          const feed = store.feed(menuFeedId)
          if (feed) store.setFeedReaderView(feed.id, !feed.readerView)
        },
      })
      if (!local) {
        const menu = await Menu.new({ items: [readerView] })
        return { menu, readerView, moves: [], resources: [readerView, menu] }
      }
      const moves: [string | null, CheckMenuItem][] = []
      for (const [id, text] of [[null, 'No Folder'], ...folders.map((f) => [f.id, f.title])])
        moves.push([id, await CheckMenuItem.new({ text: text!, action: () => store.moveFeed(menuFeedId, id) })])
      const items = [
        readerView,
        await PredefinedMenuItem.new({ item: 'Separator' }),
        await MenuItem.new({ text: 'Rename…', action: () => startRename({ kind: 'feed', id: menuFeedId }) }),
        await Submenu.new({ text: 'Move To', items: moves.map(([, item]) => item) }),
        await PredefinedMenuItem.new({ item: 'Separator' }),
        await MenuItem.new({ text: 'Unsubscribe…', action: () => void unsubscribe(menuFeedId) }),
      ]
      const menu = await Menu.new({ items })
      return { menu, readerView, moves, resources: [...moves.map(([, item]) => item), ...items, menu] }
    })()
    // A failed one is made again on the next right-click.
    const cached: Promise<FeedMenu> = menu.catch((e) => {
      if (feedMenu === cached) feedMenu = null
      throw e
    })
    feedMenu = cached
    return cached
  }

  async function showFeedMenu(e: MouseEvent, feed: Feed) {
    if (!inTauri) return
    e.preventDefault()
    // Only the latest right-click opens the menu, checked for its own feed.
    const request = ++menuRequest
    try {
      const { menu, readerView, moves } = await makeFeedMenu()
      if (request !== menuRequest) return
      await readerView.setChecked(feed.readerView)
      for (const [id, item] of moves) await item.setChecked(id === feed.folderId)
      if (request !== menuRequest) return
      menuFeedId = feed.id
      await menu.popup()
    } catch (err) {
      console.warn('Showing the feed menu failed', err)
    }
  }

  function closeAll({ resources }: { resources: { close(): Promise<void> }[] }) {
    for (const r of resources) void r.close().catch(() => {})
  }

  let folderMenu: Promise<{ menu: Menu; resources: { close(): Promise<void> }[] }> | null = null

  // The menus go with the sidebar, as it does on sign-out.
  onDestroy(() => {
    void feedMenu?.then(closeAll).catch(() => {})
    void folderMenu?.then(closeAll).catch(() => {})
  })

  async function showFolderMenu(e: MouseEvent, folderId: string) {
    if (!inTauri || !store.isLocal) return
    e.preventDefault()
    const request = ++menuRequest
    try {
      folderMenu ??= (async () => {
        const items = [
          await MenuItem.new({ text: 'Rename…', action: () => startRename({ kind: 'folder', id: menuFolderId }) }),
          await MenuItem.new({ text: 'Delete Folder…', action: () => void deleteFolder(menuFolderId) }),
        ]
        const menu = await Menu.new({ items })
        return { menu, resources: [...items, menu] }
      })().catch((err) => {
        folderMenu = null
        throw err
      })
      const { menu } = await folderMenu
      if (request !== menuRequest) return
      menuFolderId = folderId
      await menu.popup()
    } catch (err) {
      console.warn('Showing the folder menu failed', err)
    }
  }

  // Only if the account is still the one asked about when the user answers.
  async function unsubscribe(feedId: string) {
    const feed = store.feed(feedId)
    if (!feed) return
    const account = store.activeId
    const ok = await confirm(
      'Its articles are removed from this device, starred ones too.',
      `Unsubscribe from ${feed.title}?`,
      'Unsubscribe',
    )
    if (ok && store.activeId === account) store.unsubscribe(feedId)
  }

  async function deleteFolder(folderId: string) {
    const folder = store.folders.find((f) => f.id === folderId)
    if (!folder) return
    const account = store.activeId
    const ok = await confirm('Its feeds stay, in no folder.', `Delete ${folder.title}?`, 'Delete Folder')
    if (ok && store.activeId === account) store.deleteFolder(folderId)
  }

  // Renaming happens in the row itself.
  type Renamable = { kind: 'feed' | 'folder'; id: string }
  let renaming = $state<Renamable | null>(null)

  // A row that goes while renaming, such as when a sync filters it out, ends it.
  $effect(() => {
    const target = renaming
    if (target && !rows.some((r) => r.kind === target.kind && r.id === target.id)) renaming = null
  })

  // Another account's folders are its own.
  $effect(() => {
    void store.activeId
    untrack(() => {
      collapsed.clear()
      renaming = null
    })
  })

  function startRename(target: Renamable) {
    if (target.kind === 'folder') collapsed.delete(target.id)
    renaming = target
  }

  function isRenaming(scope: Scope): boolean {
    return renaming !== null && scope.kind === renaming.kind && 'id' in scope && scope.id === renaming.id
  }

  function finishRename(input: HTMLInputElement, save: boolean) {
    const target = renaming
    if (!target) return
    renaming = null
    // The row takes the focus back from the field, which goes.
    const hadFocus = document.activeElement === input
    void tick().then(() => {
      if (hadFocus) nav.querySelector<HTMLElement>(`[data-row="${CSS.escape(`${target.kind}:${target.id}`)}"]`)?.focus()
    })
    const title = input.value.trim()
    const old = target.kind === 'feed' ? store.feed(target.id)?.title : store.folders.find((f) => f.id === target.id)?.title
    if (save && title && title !== old) {
      if (target.kind === 'feed') store.renameFeed(target.id, title)
      else store.renameFolder(target.id, title)
    }
  }

  /** Focuses and selects the rename field as it appears. */
  function renameField(input: HTMLInputElement) {
    input.focus()
    input.select()
  }
</script>

{#snippet row(
  scope: Scope,
  label: string,
  count: number,
  indent = false,
  feed?: Feed,
)}
  {#if isRenaming(scope)}
    <div class="row renaming" class:indent>
      {#if feed}
        <span class="row-icon thumb">
          {#if feed.icon}<img src={feed.icon} alt="" />{:else}<Rss size={12} strokeWidth={2.5} />{/if}
        </span>
      {:else}
        <span class="row-icon"></span>
      {/if}
      <input
        class="label"
        value={label}
        aria-label="Name"
        spellcheck="false"
        use:renameField
        onblur={(e) => finishRename(e.currentTarget, true)}
        onkeydown={(e) => {
          if (e.key === 'Enter') finishRename(e.currentTarget, true)
          else if (e.key === 'Escape') finishRename(e.currentTarget, false)
          else return
          e.preventDefault()
          e.stopPropagation()
        }}
      />
    </div>
  {:else}
  <button
    class="row"
    class:indent
    data-row={'id' in scope ? `${scope.kind}:${scope.id}` : scope.kind}
    class:selected={isSelected(scope)}
    aria-current={isSelected(scope) || undefined}
    out:leave|global
    onclick={() => {
      focus.pane = 'sidebar'
      void store.select(scope)
    }}
    oncontextmenu={feed
      ? (e) => showFeedMenu(e, feed)
      : scope.kind === 'folder'
        ? (e) => showFolderMenu(e, scope.id)
        : undefined}
  >
    <span class="highlight"></span>
    {#if feed}
      <span class="row-icon thumb">
        {#if feed.icon}<img src={feed.icon} alt="" />{:else}<Rss size={12} strokeWidth={2.5} />{/if}
      </span>
    {:else if scope.kind === 'folder'}
      <!-- Room for the disclosure arrow, which sits over it. -->
      <span class="row-icon"></span>
    {/if}
    <span class="label">{label}</span>
    {#if feed?.fetchError}
      <span class="fetch-error" title="Can’t update: {feed.fetchError}">
        <CircleAlert size={13} strokeWidth={2.25} role="img" aria-label="Can’t update" />
      </span>
    {/if}
    {#if count > 0}<span class="count">{count.toLocaleString()}</span>{/if}
  </button>
  {/if}
{/snippet}

<nav class="sidebar" class:active={activePane() === 'sidebar'} aria-label="Feeds" bind:this={nav}>
  <div class="toolbar" data-tauri-drag-region={dragRegion}>
    {#if store.isLocal}<AddFeed />{/if}
    {#if store.accounts.length > 1}<AccountSwitcher />{/if}
    <!-- On macOS, ⌘, comes from the app menu, which a browser doesn't have. -->
    <button
      class="icon-button"
      aria-label="Settings"
      title={macWindow ? 'Settings (⌘,)' : isMac ? 'Settings' : 'Settings (Ctrl+,)'}
      onclick={() => openSettings()}
    >
      <SettingsIcon size={18} strokeWidth={2} />
    </button>
    <button
      class="icon-button sync"
      class:spinning={store.status.syncing}
      aria-label="Sync"
      title="Sync"
      onclick={() => store.sync()}
    >
      <RotateCw size={18} strokeWidth={2} />
    </button>
  </div>
  <header class="account" data-tauri-drag-region={dragRegion}>
    <h1>{title}</h1>
    {#if store.status.cacheBroken}
      <p class="failed" title={store.status.error ?? undefined}>
        Cache damaged ·
        <button
          class="rebuild"
          disabled={rebuilding}
          title={store.isLocal
            ? 'Starts the cache over with your feeds from their backup. Read and starred marks are lost.'
            : 'Starts the cache over and downloads the articles again. Changes not synced yet are lost.'}
          onclick={rebuild}>{rebuilding ? 'Rebuilding…' : 'Rebuild'}</button
        >
      </p>
    {:else}
      <p class:failed={store.status.error} title={store.status.error ?? undefined}>{status}</p>
    {/if}
  </header>
  <div class="scroll">
    <section>
      {@render row({ kind: 'all' }, current.allLabel, store.countTotal)}
    </section>

    {#if shownFolders.length > 0 || looseFeeds.length > 0}
      <section>
        <h2>Feeds</h2>
        {#each entries as entry (`${entry.kind}:${entry.id}`)}
          {#if entry.kind === 'feed'}
            {@render row({ kind: 'feed', id: entry.id }, entry.feed.title, store.counts.get(entry.id) ?? 0, false, entry.feed)}
          {:else}
          {@const folder = entry.folder}
          <!-- global: inside the if block, a local outro wouldn't run as the row goes. -->
          <div class="folder" out:leave|global>
            <button
              class="disclosure"
              class:open={!collapsed.has(folder.id)}
              aria-label={collapsed.has(folder.id) ? `Expand ${folder.title}` : `Collapse ${folder.title}`}
              aria-expanded={!collapsed.has(folder.id)}
              onclick={() => toggleFolder(folder.id)}
            >
              <ChevronRight size={16} strokeWidth={2} />
            </button>
            {@render row({ kind: 'folder', id: folder.id }, folder.title, store.folderCounts.get(folder.id) ?? 0)}
          </div>
          {#if !collapsed.has(folder.id)}
            <div transition:slide={{ duration: ms(160) }}>
              {#each feedsByFolder.get(folder.id) ?? [] as feed (feed.id)}
                {@render row({ kind: 'feed', id: feed.id }, feed.title, store.counts.get(feed.id) ?? 0, true, feed)}
              {/each}
            </div>
          {/if}
          {/if}
        {/each}
      </section>
    {/if}
  </div>
  <div
    class="views"
    role="radiogroup"
    aria-label="Show"
    tabindex="-1"
    style:--count={views.length}
    style:--index={viewIndex}
    onkeydown={onViewsKeydown}
  >
    <span class="indicator" aria-hidden="true"></span>
    {#each views as v, i (v.view)}
      <button
        role="radio"
        aria-label={v.label}
        title={v.label}
        aria-checked={store.view === v.view}
        tabindex={store.view === v.view ? 0 : -1}
        bind:this={viewButtons[i]}
        onclick={(e) => {
          // A clicked button keeps the focus on Linux, and with it ← and →. Tab still
          // reaches the switcher.
          if (e.detail > 0) e.currentTarget.blur()
          store.setView(v.view)
        }}
      >
        <v.icon size={16} strokeWidth={2} />
      </button>
    {/each}
  </div>
</nav>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    height: 100%;
    font-size: 14px;
    background: var(--sidebar-bg);
  }

  /* Over the header, for the account menu. */
  .toolbar {
    position: relative;
    z-index: 2;
    justify-content: flex-end;
  }

  .sync.spinning :global(svg) {
    animation: spin 0.9s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* Without the spin, a dimmed icon shows the sync. */
  :global(:root[data-motion='off']) .sync.spinning :global(svg) {
    opacity: 0.5;
  }

  .account {
    padding: 6px 22px 18px;
  }

  .account h1 {
    margin: 0;
    font-size: 30px;
    font-weight: 800;
    line-height: 1.1;
    letter-spacing: -0.02em;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .account p {
    margin: 3px 0 0;
    font-weight: 600;
    color: var(--text-3);
  }

  .account p.failed {
    color: var(--error);
  }

  .rebuild {
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    color: inherit;
    text-decoration: underline;
    cursor: pointer;
  }

  .rebuild:disabled {
    text-decoration: none;
    cursor: default;
  }

  .scroll {
    flex: 1;
    overflow-y: auto;
    /* The row highlights' stacking context: under every row's text, over the background. */
    isolation: isolate;
    padding: 0 8px 12px;
  }

  section + section {
    margin-top: 18px;
  }

  /* Lined up with the account header and the All row. */
  h2 {
    margin: 0 0 4px;
    padding: 0 14px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-3);
  }

  .folder {
    position: relative;
  }

  /* Sits over the folder row's icon slot. */
  .disclosure {
    position: absolute;
    left: 6px;
    top: 0;
    bottom: 0;
    z-index: 1;
    display: grid;
    place-items: center;
    width: 34px;
    color: inherit;
  }

  .active .folder:has(.row.selected) .disclosure {
    color: var(--accent-text);
  }

  .disclosure :global(svg) {
    transition: transform 120ms ease;
  }

  .disclosure.open :global(svg) {
    transform: rotate(90deg);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 32px;
    padding: 0 10px 0 14px;
    border-radius: 6px;
    text-align: left;
    color: var(--text);
    position: relative;
    transition: color 120ms ease-out;
  }

  /* Lines the feed's icon up with its folder's title. */
  .row.indent {
    padding-left: 40px;
  }

  .row:hover {
    background: var(--hover);
  }

  /* The selection's background, behind every row's text (see .scroll), so it can slide
     past other rows. */
  .highlight {
    position: absolute;
    inset: 0;
    z-index: -1;
    border-radius: inherit;
  }

  /* The tint lowers the contrast of dimmer text, so it takes the full text color. */
  .row.selected .highlight {
    background: var(--accent-muted);
  }

  /* The highlight shows the focused selection; a ring would show ahead of its slide.
     Forced colors (High Contrast) override the color, so the ring shows there. */
  .row.selected:focus-visible {
    outline-color: transparent;
  }

  .row.selected .count {
    color: var(--text);
  }

  .active .row.selected {
    color: var(--accent-text);
  }

  .active .row.selected .highlight {
    background: var(--accent);
  }

  .row :global(.row-icon) {
    flex: none;
    width: 18px;
    height: 18px;
  }

  .active .row.selected .count {
    color: inherit;
  }

  /* A feed's favicon, or the RSS icon, on a tile. */
  .thumb {
    display: grid;
    place-items: center;
    overflow: hidden;
    border-radius: 4px;
    background: var(--thumb-bg);
    box-shadow: 0 0 0 0.5px var(--thumb-edge);
    color: var(--text-2);
  }

  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .label {
    flex: 1;
    font-weight: 500;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .count,
  .disclosure {
    transition: color 120ms ease-out;
  }

  .count {
    flex: none;
    color: var(--text-3);
  }

  /* A feed the last refresh couldn't fetch. */
  .fetch-error {
    display: grid;
    flex: none;
    color: var(--error);
  }

  .active .row.selected .fetch-error {
    color: inherit;
  }

  input.label {
    height: 24px;
    margin-left: -6px;
    padding: 0 5px;
    border: 0;
    border-radius: 5px;
    background: var(--reader-bg);
    font: inherit;
    font-weight: 500;
    color: var(--text);
    outline: 2px solid var(--accent);
  }

  /* A segmented control: one button per view, over an indicator that slides to the
     selected one. */
  .views {
    position: relative;
    flex: none;
    display: flex;
    gap: 2px;
    margin: 8px 8px 12px;
    padding: 2px;
    border-radius: 8px;
    background: var(--active);
  }

  .indicator {
    position: absolute;
    top: 2px;
    bottom: 2px;
    left: 2px;
    width: calc((100% - 4px - (var(--count) - 1) * 2px) / var(--count));
    transform: translateX(calc(var(--index) * (100% + 2px)));
    border-radius: 6px;
    background: var(--segment-bg);
    box-shadow:
      0 0 0 0.5px var(--thumb-edge),
      0 0.5px 2px rgb(0 0 0 / 0.15);
    transition: transform 180ms ease;
  }

  .views button {
    position: relative;
    flex: 1;
    display: grid;
    place-items: center;
    height: 24px;
    border-radius: 6px;
    color: var(--text-2);
  }

  .views button:hover:not([aria-checked='true']) {
    background: var(--hover);
    color: var(--text);
  }

  .views button[aria-checked='true'] {
    color: var(--text);
  }
</style>
