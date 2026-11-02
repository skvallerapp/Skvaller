<script lang="ts">
  import { onMount } from 'svelte'
  import ArticleList from './lib/ArticleList.svelte'
  import { fontStacks } from './lib/fonts'
  import { LIST, SIDEBAR, layout, saveLayout, toggleSidebar } from './lib/layout.svelte'
  import Reader from './lib/Reader.svelte'
  import Resizer from './lib/Resizer.svelte'
  import Sidebar from './lib/Sidebar.svelte'
  import SignIn from './lib/SignIn.svelte'
  import { store } from './lib/store.svelte'

  onMount(() => void store.init())

  $effect(() => {
    document.documentElement.style.fontFamily = fontStacks[store.settings.uiFont]
  })

  let resizing = $state(false)

  function onkeydown(e: KeyboardEvent) {
    // Cmd+Shift+S on macOS, Ctrl+Shift+S elsewhere.
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.code === 'KeyS') {
      e.preventDefault()
      toggleSidebar()
    }
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
      <ArticleList />
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
      <Reader />
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
