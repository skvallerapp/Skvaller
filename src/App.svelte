<script lang="ts">
  import { onMount } from 'svelte'
  import ArticleList from './lib/ArticleList.svelte'
  import { styleRoot } from './lib/theme'
  import { openInBrowser, openSettings } from './lib/api'
  import { LIST, SIDEBAR, activePane, focus, layout, saveLayout, toggleSidebar } from './lib/layout.svelte'
  import Reader from './lib/Reader.svelte'
  import Resizer from './lib/Resizer.svelte'
  import Sidebar from './lib/Sidebar.svelte'
  import AccountSwitcher from './lib/AccountSwitcher.svelte'
  import LocalOption from './lib/LocalOption.svelte'
  import SignIn from './lib/SignIn.svelte'
  import { isMac } from './lib/platform'
  import { arrowKeys, bindings, keyName } from './lib/shortcuts'
  import { store } from './lib/store.svelte'
  import type { ShortcutAction } from './lib/types'

  onMount(() => void store.init())

  $effect(() => styleRoot(store.settings))

  let resizing = $state(false)

  let sidebar = $state<Sidebar>()
  let list = $state<ArticleList>()
  let reader = $state<Reader>()

  const actions: Record<ShortcutAction, () => void> = {
    next: () => stepList(1),
    previous: () => stepList(-1),
    readOn: () => reader?.readOn(1),
    readBack: () => reader?.readOn(-1),
    toggleRead: () => store.article && store.toggleRead(store.article.id),
    toggleStar: () => store.article && store.toggleStarred(store.article.id),
    openInBrowser: () => store.article && openInBrowser(store.article.url),
    toggleReaderView: () => store.toggleReaderView(),
    sync: () => store.sync(),
    markAllRead: () => list?.markAllRead(),
  }
  /** j and k move in the list from anywhere, so the list takes the focus. */
  function stepList(delta: 1 | -1) {
    focus.pane = 'list'
    store.step(delta)
  }

  /** ↑ and ↓ move in the pane in focus. ← and → move the focus; the list, on getting it,
   * opens its first article if none is open, so the focus shows. */
  function onArrow(key: string) {
    // A focused row stays with its pane: Enter mustn't act on the pane left behind.
    if (key === 'ArrowLeft' || key === 'ArrowRight')
      (document.activeElement as HTMLElement | null)?.closest<HTMLElement>('.row')?.blur()
    if (key === 'ArrowLeft') {
      if (layout.sidebarOpen) focus.pane = 'sidebar'
    } else if (key === 'ArrowRight') {
      focus.pane = 'list'
      // Right after a sidebar step, the new list may still be loading.
      if (!store.article) void store.listShown.then(() => store.article || store.step(1))
    } else {
      const delta = key === 'ArrowDown' ? 1 : -1
      if (activePane() === 'sidebar') void sidebar?.step(delta)
      else stepList(delta)
    }
  }

  /** Actions that run again while their key is held down. The others run once. */
  const repeating = new Set<ShortcutAction>(['next', 'previous', 'readOn', 'readBack'])

  const actionByKey = $derived.by(() => {
    const map = new Map<string, ShortcutAction>()
    for (const [action, key] of Object.entries(bindings(store.settings.shortcuts)))
      if (key !== null) map.set(key, action as ShortcutAction)
    return map
  })

  /** True when the key belongs to the focused element: any key in a text field or a
   * dialog, and arrow keys on a resizer, in the view switcher or while reading, which
   * scrolls the article. A clicked view button keeps the focus, so it mustn't take
   * every key. */
  function ownsKey(e: KeyboardEvent): boolean {
    const target = e.target as Element
    if (target.closest?.('input, textarea, select, [contenteditable], [role=dialog], [role=menu]')) return true
    if (!e.key.startsWith('Arrow')) return false
    return target.ownerDocument !== document || !!target.closest?.('[role=separator], [role=radiogroup]')
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
    if (key !== null && arrowKeys.has(key)) {
      e.preventDefault()
      // ↑ and ↓ repeat in the list only: each sidebar step loads a whole list.
      const repeats = activePane() === 'list' && (key === 'ArrowUp' || key === 'ArrowDown')
      if (!e.repeat || repeats) onArrow(key)
      return
    }
    const action = key === null ? undefined : actionByKey.get(key)
    if (!action) return
    e.preventDefault()
    if (!e.repeat || repeating.has(action)) actions[action]()
  }
</script>

<svelte:window {onkeydown} />

{#if !store.ready}
  <!-- Blank until the session is known, so the sign-in screen doesn't flash. -->
{:else if !store.signedIn}
  <!-- For an account whose token the server rejected, filled in; or the first account. -->
  {#key store.activeId}
    <SignIn
      submit={(server, username, password) => store.signIn(server, username, password)}
      account={store.account?.kind === 'greader' ? store.account : null}
      initialError={store.status.unauthorized
        ? `${store.account?.description} no longer accepts the sign-in. Sign in again.`
        : store.status.error}
    >
      <!-- Not to someone signing in again. -->
      {#if !store.account && !store.accounts.some((a) => a.kind === 'local')}
        <LocalOption add={() => store.addLocalAccount()} />
      {/if}
    </SignIn>
  {/key}
  {#if store.accounts.length > 1}
    <!-- The other accounts still work. -->
    <div class="other-accounts"><AccountSwitcher /></div>
  {/if}
{:else}
  <div class="app" class:resizing>
    <div
      class="sidebar"
      class:closed={!layout.sidebarOpen}
      style:width="{layout.sidebarOpen ? layout.sidebarWidth : 0}px"
      inert={!layout.sidebarOpen}
    >
      <div class="sidebar-inner" style:width="{layout.sidebarWidth}px">
        <Sidebar bind:this={sidebar} />
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
      <ArticleList bind:this={list} onmarkedall={() => sidebar?.selectNext()} />
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
  .other-accounts {
    position: fixed;
    top: 8px;
    right: 10px;
    width: 260px;
    display: flex;
    justify-content: flex-end;
  }

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
