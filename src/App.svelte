<script lang="ts">
  import { onMount } from 'svelte'
  import ArticleList from './lib/ArticleList.svelte'
  import { fontStacks } from './lib/fonts'
  import { openInBrowser, openSettings } from './lib/api'
  import { LIST, SIDEBAR, layout, saveLayout, toggleSidebar } from './lib/layout.svelte'
  import Reader from './lib/Reader.svelte'
  import Resizer from './lib/Resizer.svelte'
  import Sidebar from './lib/Sidebar.svelte'
  import SignIn from './lib/SignIn.svelte'
  import { isMac } from './lib/platform'
  import { bindings, fixedKeys, keyName } from './lib/shortcuts'
  import { store } from './lib/store.svelte'
  import type { ShortcutAction } from './lib/types'

  onMount(() => void store.init())

  $effect(() => {
    document.documentElement.style.fontFamily = fontStacks[store.settings.uiFont]
  })

  let resizing = $state(false)

  let list = $state<ArticleList>()
  let reader = $state<Reader>()

  const actions: Record<ShortcutAction, () => void> = {
    next: () => store.step(1),
    previous: () => store.step(-1),
    readOn: () => reader?.readOn(1),
    readBack: () => reader?.readOn(-1),
    toggleRead: () => store.article && store.toggleRead(store.article.id),
    toggleStar: () => store.article && store.toggleStarred(store.article.id),
    openInBrowser: () => store.article && openInBrowser(store.article.url),
    toggleReaderView: () => store.toggleReaderView(),
    sync: () => store.sync(),
    markAllRead: () => list?.markAllRead(),
  }
  /** Actions that run again while their key is held down. The others run once. */
  const repeating = new Set<ShortcutAction>(['next', 'previous', 'readOn', 'readBack'])

  const actionByKey = $derived.by(() => {
    const map = new Map<string, ShortcutAction>()
    for (const [action, key] of Object.entries(bindings(store.settings.shortcuts)))
      if (key !== null) map.set(key, action as ShortcutAction)
    return map
  })

  /** True when the key belongs to the focused element: a text field, a dialog, a
   * resizer, the view switcher, or arrow keys while reading, which scroll the article. */
  function ownsKey(e: KeyboardEvent): boolean {
    const target = e.target as Element
    if (target.closest?.('input, textarea, select, [contenteditable], [role=dialog], [role=separator], [role=radiogroup]'))
      return true
    return e.key.startsWith('Arrow') && target.ownerDocument !== document
  }

  // Keys the reader frame receives come here too.
  function onkeydown(e: KeyboardEvent) {
    // Cmd+Shift+S on macOS, Ctrl+Shift+S elsewhere.
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.code === 'KeyS') {
      e.preventDefault()
      toggleSidebar()
      return
    }
    // macOS opens Settings from the app menu, with Cmd+,.
    if (!isMac && e.ctrlKey && !e.shiftKey && !e.altKey && e.key === ',') {
      e.preventDefault()
      void openSettings()
      return
    }
    if (!store.signedIn || ownsKey(e)) return
    const key = keyName(e)
    const action = key === null ? undefined : (fixedKeys[key] ?? actionByKey.get(key))
    if (!action) return
    e.preventDefault()
    if (!e.repeat || repeating.has(action)) actions[action]()
  }
</script>

<svelte:window {onkeydown} />

{#if !store.ready}
  <!-- Blank until the session is known, so the sign-in screen doesn't flash. -->
{:else if !store.signedIn}
  <SignIn />
{:else}
  <div class="app" class:resizing>
    <div
      class="sidebar"
      class:closed={!layout.sidebarOpen}
      style:width="{layout.sidebarOpen ? layout.sidebarWidth : 0}px"
      inert={!layout.sidebarOpen}
    >
      <div class="sidebar-inner" style:width="{layout.sidebarWidth}px">
        <Sidebar />
      </div>
    </div>
    {#if layout.sidebarOpen}
      <Resizer
        label="Resize sidebar"
        value={layout.sidebarWidth}
        min={SIDEBAR.min}
        max={SIDEBAR.max}
        onchange={(w) => (layout.sidebarWidth = w)}
        onend={saveLayout}
        ondragging={(d) => (resizing = d)}
      />
    {/if}

    <div class="list" style:width="{layout.listWidth}px">
      <ArticleList bind:this={list} />
    </div>
    <Resizer
      label="Resize article list"
      value={layout.listWidth}
      min={LIST.min}
      max={LIST.max}
      onchange={(w) => (layout.listWidth = w)}
      onend={saveLayout}
      ondragging={(d) => (resizing = d)}
    />

    <div class="reader">
      <Reader bind:this={reader} {onkeydown} />
    </div>
  </div>
{/if}

<style>
  .app {
    display: flex;
    height: 100%;
  }

  .app.resizing {
    cursor: col-resize;
    user-select: none;
  }

  /* While dragging, the frame would swallow pointer events. */
  .app.resizing :global(iframe) {
    pointer-events: none;
  }

  .sidebar {
    flex: none;
    overflow: hidden;
    transition: width 180ms ease;
  }

  .app.resizing .sidebar {
    transition: none;
  }

  .sidebar-inner {
    height: 100%;
  }

  .list {
    flex: none;
    min-width: 0;
  }

  .reader {
    flex: 1;
    min-width: 0;
  }
</style>
