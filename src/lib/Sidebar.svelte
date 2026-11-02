<script lang="ts">
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import CircleDot from '@lucide/svelte/icons/circle-dot'
  import Layers from '@lucide/svelte/icons/layers'
  import RotateCw from '@lucide/svelte/icons/rotate-cw'
  import Rss from '@lucide/svelte/icons/rss'
  import Star from '@lucide/svelte/icons/star'
  import { SvelteSet } from 'svelte/reactivity'
  import { formatSynced, serviceTitle } from './format'
  import { dragRegion } from './platform'
  import { store } from './store.svelte'
  import type { Feed, Selection, SmartView } from './types'

  const smartViews: { view: SmartView; label: string; icon: typeof Star }[] = [
    { view: 'unread', label: 'Unread', icon: CircleDot },
    { view: 'starred', label: 'Starred', icon: Star },
    { view: 'all', label: 'All Articles', icon: Layers },
  ]

  const collapsed = new SvelteSet<string>()
  const looseFeeds = $derived(store.sidebarFeeds.filter((f) => f.folderId === null))
  const title = $derived(store.account ? serviceTitle(store.account.server) : '')
  const status = $derived.by(() => {
    const s = store.status
    if (s.syncing) return 'Syncing…'
    if (s.error) return 'Sync failed'
    return s.lastSync ? formatSynced(s.lastSync) : 'Not synced yet'
  })

  function isSelected(sel: Selection): boolean {
    const cur = store.selection
    if (cur.kind !== sel.kind) return false
    return cur.kind === 'smart' ? cur.view === (sel as typeof cur).view : cur.id === (sel as typeof cur).id
  }

  function toggleFolder(id: string) {
    if (!collapsed.delete(id)) collapsed.add(id)
  }
</script>

{#snippet row(
  sel: Selection,
  label: string,
  count: number,
  Icon: typeof Star | null,
  indent = false,
  feed?: Feed,
)}
  <button
    class="row"
    class:indent
    class:selected={isSelected(sel)}
    onclick={() => store.select(sel)}
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
    {#if count > 0}<span class="count">{count}</span>{/if}
  </button>
{/snippet}

<nav class="sidebar" aria-label="Feeds">
  <div class="toolbar" data-tauri-drag-region={dragRegion}>
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
      {#each smartViews as v (v.view)}
        {@render row(
          { kind: 'smart', view: v.view },
          v.label,
          v.view === 'unread' ? store.unreadTotal : v.view === 'starred' ? store.starredCount : 0,
          v.icon,
        )}
      {/each}
    </section>

    <section>
      <h2>Feeds</h2>
      {#each store.folders as folder (folder.id)}
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
          {@render row({ kind: 'folder', id: folder.id }, folder.title, store.folderUnread(folder.id), null)}
        </div>
        {#if !collapsed.has(folder.id)}
          {#each store.sidebarFeeds.filter((f) => f.folderId === folder.id) as feed (feed.id)}
            {@render row({ kind: 'feed', id: feed.id }, feed.title, store.unreadByFeed.get(feed.id) ?? 0, null, true, feed)}
          {/each}
        {/if}
      {/each}
      {#each looseFeeds as feed (feed.id)}
        {@render row({ kind: 'feed', id: feed.id }, feed.title, store.unreadByFeed.get(feed.id) ?? 0, null, false, feed)}
      {/each}
    </section>
  </div>
</nav>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    height: 100%;
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
    font-size: 13px;
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
</style>
