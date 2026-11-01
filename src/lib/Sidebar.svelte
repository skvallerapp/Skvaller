<script lang="ts">
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import CircleDot from '@lucide/svelte/icons/circle-dot'
  import Folder from '@lucide/svelte/icons/folder'
  import Layers from '@lucide/svelte/icons/layers'
  import Rss from '@lucide/svelte/icons/rss'
  import Star from '@lucide/svelte/icons/star'
  import { SvelteSet } from 'svelte/reactivity'
  import { store } from './store.svelte'
  import type { Selection, SmartView } from './types'

  const smartViews: { view: SmartView; label: string; icon: typeof Star }[] = [
    { view: 'unread', label: 'Unread', icon: CircleDot },
    { view: 'starred', label: 'Starred', icon: Star },
    { view: 'all', label: 'All Articles', icon: Layers },
  ]

  const collapsed = new SvelteSet<string>()
  const looseFeeds = store.feeds.filter((f) => f.folderId === null)

  function isSelected(sel: Selection): boolean {
    const cur = store.selection
    if (cur.kind !== sel.kind) return false
    return cur.kind === 'smart' ? cur.view === (sel as typeof cur).view : cur.id === (sel as typeof cur).id
  }

  function toggleFolder(id: string) {
    if (!collapsed.delete(id)) collapsed.add(id)
  }
</script>

{#snippet row(sel: Selection, label: string, count: number, Icon: typeof Star, indent = false)}
  <button
    class="row"
    class:indent
    class:selected={isSelected(sel)}
    onclick={() => store.select(sel)}
  >
    <Icon size={15} strokeWidth={1.75} class="row-icon" />
    <span class="label">{label}</span>
    {#if count > 0}<span class="count">{count}</span>{/if}
  </button>
{/snippet}

<nav class="sidebar" aria-label="Feeds">
  <div class="toolbar"></div>
  <div class="scroll">
    <section>
      {#each smartViews as v (v.view)}
        {@render row(
          { kind: 'smart', view: v.view },
          v.label,
          v.view === 'unread' ? store.unreadTotal : v.view === 'starred' ? store.starred.size : 0,
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
            <ChevronRight size={12} strokeWidth={2.25} />
          </button>
          {@render row({ kind: 'folder', id: folder.id }, folder.title, store.folderUnread(folder.id), Folder)}
        </div>
        {#if !collapsed.has(folder.id)}
          {#each store.feeds.filter((f) => f.folderId === folder.id) as feed (feed.id)}
            {@render row({ kind: 'feed', id: feed.id }, feed.title, store.unreadByFeed.get(feed.id) ?? 0, Rss, true)}
          {/each}
        {/if}
      {/each}
      {#each looseFeeds as feed (feed.id)}
        {@render row({ kind: 'feed', id: feed.id }, feed.title, store.unreadByFeed.get(feed.id) ?? 0, Rss)}
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

  .disclosure {
    position: absolute;
    left: -2px;
    top: 0;
    bottom: 0;
    z-index: 1;
    display: grid;
    place-items: center;
    width: 14px;
    color: var(--text-3);
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
    height: 28px;
    padding: 0 10px 0 14px;
    border-radius: 6px;
    text-align: left;
    color: var(--text);
  }

  .row.indent {
    padding-left: 30px;
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
    color: var(--text-2);
  }

  .row.selected :global(.row-icon),
  .row.selected .count {
    color: inherit;
  }

  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .count {
    flex: none;
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    color: var(--text-3);
  }
</style>
