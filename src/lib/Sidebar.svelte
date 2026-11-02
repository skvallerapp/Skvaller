<script lang="ts">
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import RotateCw from '@lucide/svelte/icons/rotate-cw'
  import SettingsIcon from '@lucide/svelte/icons/settings'
  import Rss from '@lucide/svelte/icons/rss'
  import { CheckMenuItem, Menu } from '@tauri-apps/api/menu'
  import { untrack } from 'svelte'
  import { SvelteSet } from 'svelte/reactivity'
  import { formatSynced, serviceTitle } from './format'
  import { openSettings } from './api'
  import { dragRegion, inTauri, isMac } from './platform'
  import { store } from './store.svelte'
  import type { Feed, Scope } from './types'
  import { type ViewIcon, viewInfo, views } from './views'

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
  const title = $derived(store.account ? serviceTitle(store.account.server) : '')
  const status = $derived.by(() => {
    const s = store.status
    if (s.syncing) return 'Syncing…'
    if (s.error) return 'Sync failed'
    return s.lastSync ? formatSynced(s.lastSync) : 'Not synced yet'
  })

  function isSelected(scope: Scope): boolean {
    const cur = store.scope
    if (cur.kind !== scope.kind) return false
    return cur.kind === 'all' || cur.id === (scope as typeof cur).id
  }

  let viewButtons: HTMLButtonElement[] = []

  /** Arrow keys move between the views, as in a radio group. */
  function onViewsKeydown(e: KeyboardEvent) {
    const step = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 }[e.key]
    if (!step) return
    e.preventDefault()
    const i = (views.findIndex((v) => v.view === store.view) + step + views.length) % views.length
    store.setView(views[i].view)
    viewButtons[i].focus()
  }

  function toggleFolder(id: string) {
    if (!collapsed.delete(id)) collapsed.add(id)
  }

  // One native menu for every feed, made on first use: menus are app resources that
  // stay until closed, and on Linux `popup` returns before the menu does.
  let feedMenu: Promise<{ menu: Menu; readerView: CheckMenuItem }> | null = null
  // The feed the menu was last opened for. Its action acts on this one.
  let menuFeedId = ''
  let menuRequest = 0

  function makeFeedMenu() {
    feedMenu ??= (async () => {
      const readerView = await CheckMenuItem.new({
        text: 'Always Use Reader View',
        action: () => {
          const feed = store.feed(menuFeedId)
          if (feed) store.setFeedReaderView(feed.id, !feed.readerView)
        },
      })
      return { menu: await Menu.new({ items: [readerView] }), readerView }
    })().catch((e) => {
      feedMenu = null
      throw e
    })
    return feedMenu
  }

  async function showFeedMenu(e: MouseEvent, feed: Feed) {
    if (!inTauri) return
    e.preventDefault()
    // Only the latest right-click opens the menu, checked for its own feed.
    const request = ++menuRequest
    try {
      const { menu, readerView } = await makeFeedMenu()
      if (request !== menuRequest) return
      await readerView.setChecked(feed.readerView)
      if (request !== menuRequest) return
      menuFeedId = feed.id
      await menu.popup()
    } catch (err) {
      console.warn('Showing the feed menu failed', err)
    }
  }
</script>

{#snippet row(
  scope: Scope,
  label: string,
  count: number,
  Icon: ViewIcon | null,
  indent = false,
  feed?: Feed,
)}
  <button
    class="row"
    class:indent
    class:selected={isSelected(scope)}
    onclick={() => store.select(scope)}
    oncontextmenu={feed ? (e) => showFeedMenu(e, feed) : undefined}
  >
    {#if feed}
      <span class="row-icon thumb">
        {#if feed.icon}<img src={feed.icon} alt="" />{:else}<Rss size={12} strokeWidth={2.5} />{/if}
      </span>
    {:else if Icon}
      <Icon size={18} strokeWidth={2} class="row-icon" />
    {:else}
      <span class="row-icon"></span>
    {/if}
    <span class="label">{label}</span>
    {#if count > 0}<span class="count">{count.toLocaleString()}</span>{/if}
  </button>
{/snippet}

<nav class="sidebar" aria-label="Feeds">
  <div class="toolbar" data-tauri-drag-region={dragRegion}>
    {#if !isMac || !inTauri}
      <!-- macOS opens Settings from the app menu, which a browser doesn't have. -->
      <button
        class="icon-button"
        aria-label="Settings"
        title={isMac ? 'Settings' : 'Settings (Ctrl+,)'}
        onclick={() => openSettings()}
      >
        <SettingsIcon size={18} strokeWidth={2} />
      </button>
    {/if}
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
    <p class:failed={store.status.error} title={store.status.error ?? undefined}>{status}</p>
  </header>
  <div class="scroll">
    <section>
      {@render row({ kind: 'all' }, current.allLabel, store.countTotal, current.icon)}
    </section>

    {#if shownFolders.length > 0 || looseFeeds.length > 0}
      <section>
        <h2>Feeds</h2>
        {#each shownFolders as folder (folder.id)}
          <div class="folder">
            <button
              class="disclosure"
              class:open={!collapsed.has(folder.id)}
              aria-label={collapsed.has(folder.id) ? `Expand ${folder.title}` : `Collapse ${folder.title}`}
              aria-expanded={!collapsed.has(folder.id)}
              onclick={() => toggleFolder(folder.id)}
            >
              <ChevronRight size={16} strokeWidth={2} />
            </button>
            {@render row({ kind: 'folder', id: folder.id }, folder.title, store.folderCounts.get(folder.id) ?? 0, null)}
          </div>
          {#if !collapsed.has(folder.id)}
            {#each feedsByFolder.get(folder.id) ?? [] as feed (feed.id)}
              {@render row({ kind: 'feed', id: feed.id }, feed.title, store.counts.get(feed.id) ?? 0, null, true, feed)}
            {/each}
          {/if}
        {/each}
        {#each looseFeeds as feed (feed.id)}
          {@render row({ kind: 'feed', id: feed.id }, feed.title, store.counts.get(feed.id) ?? 0, null, false, feed)}
        {/each}
      </section>
    {/if}
  </div>
  <div class="views" role="radiogroup" aria-label="Show" tabindex="-1" onkeydown={onViewsKeydown}>
    {#each views as v, i (v.view)}
      <button
        role="radio"
        aria-label={v.label}
        title={v.label}
        aria-checked={store.view === v.view}
        tabindex={store.view === v.view ? 0 : -1}
        bind:this={viewButtons[i]}
        onclick={() => store.setView(v.view)}
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

  .toolbar {
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

  @media (prefers-reduced-motion: reduce) {
    .sync.spinning :global(svg) {
      animation: none;
      opacity: 0.5;
    }
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

  .scroll {
    flex: 1;
    overflow-y: auto;
    padding: 0 8px 12px;
  }

  section + section {
    margin-top: 18px;
  }

  h2 {
    margin: 0 0 4px;
    padding: 0 10px;
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

  .folder:has(.row.selected) .disclosure {
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
  }

  /* Lines the feed's icon up with its folder's title. */
  .row.indent {
    padding-left: 40px;
  }

  .row:hover {
    background: var(--hover);
  }

  .row.selected {
    background: var(--accent);
    color: var(--accent-text);
  }

  .row :global(.row-icon) {
    flex: none;
    width: 18px;
    height: 18px;
  }

  .row.selected .count {
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

  .count {
    flex: none;
    color: var(--text-3);
  }

  /* A segmented control: one button per view. */
  .views {
    flex: none;
    display: flex;
    gap: 2px;
    margin: 8px 8px 12px;
    padding: 2px;
    border-radius: 8px;
    background: var(--active);
  }

  .views button {
    flex: 1;
    display: grid;
    place-items: center;
    height: 24px;
    border-radius: 6px;
    color: var(--text-2);
  }

  .views button:hover {
    background: var(--hover);
    color: var(--text);
  }

  .views button[aria-checked='true'] {
    background: var(--segment-bg);
    box-shadow:
      0 0 0 0.5px var(--thumb-edge),
      0 0.5px 2px rgb(0 0 0 / 0.15);
    color: var(--text);
  }
</style>
