<script lang="ts">
  import Check from '@lucide/svelte/icons/check'
  import Plus from '@lucide/svelte/icons/plus'
  import Rss from '@lucide/svelte/icons/rss'
  import { tick } from 'svelte'
  import { openSettings } from './api'
  import { store } from './store.svelte'

  // Closes on Escape, a pick, a click or focus elsewhere, like the mark-all-read popover.
  let open = $state(false)
  let root: HTMLElement
  let trigger: HTMLButtonElement
  let menu = $state<HTMLElement>()

  /** The active account's live count: its articles are being read right now. */
  const unread = (id: string, saved: number) =>
    id === store.activeId && store.signedIn
      ? [...store.unreadByFeed.values()].reduce((sum, n) => sum + n, 0)
      : saved

  async function toggle() {
    open = !open
    if (!open) return
    void store.refreshAccounts().catch((e) => console.warn('Loading the accounts failed', e))
    await tick()
    const items = menuItems()
    ;(items.find((i) => i.getAttribute('aria-checked') === 'true') ?? items[0])?.focus()
  }

  function close() {
    open = false
    trigger.focus()
  }

  function pick(id: string) {
    close()
    void store.switchAccount(id).catch((e) => console.error('Switching accounts failed', e))
  }

  function addAccount() {
    close()
    void openSettings('add-account')
  }

  function menuItems(): HTMLElement[] {
    return menu ? [...menu.querySelectorAll<HTMLElement>('[role^="menuitem"]')] : []
  }

  function onMenuKeydown(e: KeyboardEvent) {
    const items = menuItems()
    const i = items.indexOf(document.activeElement as HTMLElement)
    const to = (n: number) => {
      e.preventDefault()
      items[(n + items.length) % items.length]?.focus()
    }
    if (e.key === 'ArrowDown') to(i + 1)
    else if (e.key === 'ArrowUp') to(i < 0 ? -1 : i - 1)
    else if (e.key === 'Home') to(0)
    else if (e.key === 'End') to(-1)
    else if (e.key === 'Escape' || e.key === 'Tab') {
      e.preventDefault()
      close()
    } else return
    // The app's own shortcuts don't see keys the menu used.
    e.stopPropagation()
  }

  function onWindowPointerdown(e: PointerEvent) {
    if (open && !root.contains(e.target as Node)) open = false
  }

  // WebKit doesn't focus a clicked button, so only focus moving to another element
  // closes the menu here; clicks outside close it in onWindowPointerdown.
  function onFocusout(e: FocusEvent) {
    const next = e.relatedTarget as Node | null
    if (next && !root.contains(next)) open = false
  }
</script>

<svelte:window onpointerdown={onWindowPointerdown} />

<div class="switcher" bind:this={root} onfocusout={onFocusout}>
  <button
    class="icon-button"
    bind:this={trigger}
    aria-label="Switch Account"
    title="Switch Account"
    aria-haspopup="menu"
    aria-expanded={open}
    onclick={toggle}
  >
    <Rss size={18} strokeWidth={2} />
  </button>
  {#if open}
    <!-- svelte-ignore a11y_interactive_supports_focus -->
    <div
      class="menu"
      role="menu"
      aria-label="Accounts"
      data-tauri-drag-region="false"
      bind:this={menu}
      onkeydown={onMenuKeydown}
    >
      {#each store.accounts as account (account.id)}
        {@const count = unread(account.id, account.unread)}
        <button
          class="item"
          role="menuitemradio"
          aria-checked={account.id === store.activeId}
          tabindex="-1"
          onclick={() => pick(account.id)}
        >
          <span class="check">
            {#if account.id === store.activeId}<Check size={14} strokeWidth={2.5} />{/if}
          </span>
          <span class="name">{account.description}</span>
          {#if count > 0}<span class="count">{count.toLocaleString()}</span>{/if}
        </button>
      {/each}
      <div class="separator" role="separator"></div>
      <button class="item" role="menuitem" tabindex="-1" onclick={addAccount}>
        <span class="check"><Plus size={14} strokeWidth={2.5} /></span>
        <span class="name">Add Account…</span>
      </button>
    </div>
  {/if}
</div>

<style>
  /* The menu spans the sidebar, whose edges clip anything wider. */
  .menu {
    position: absolute;
    top: calc(100% - 4px);
    left: 8px;
    right: 8px;
    z-index: 10;
    padding: 5px;
    border: 1px solid var(--separator);
    border-radius: 10px;
    background: var(--reader-bg);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.18);
    animation: appear 140ms ease-out;
    transform-origin: top;
  }

  .item {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    height: 28px;
    padding: 0 8px 0 4px;
    border-radius: 6px;
    text-align: left;
    color: var(--text);
  }

  .item:hover,
  .item:focus-visible {
    background: var(--accent);
    color: var(--accent-text);
    outline: none;
  }

  .check {
    display: grid;
    place-items: center;
    flex: none;
    width: 18px;
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-weight: 500;
  }

  .count {
    flex: none;
    font-variant-numeric: tabular-nums;
    color: var(--text-3);
  }

  .item:hover .count,
  .item:focus-visible .count {
    color: inherit;
  }

  .separator {
    height: 1px;
    margin: 5px 4px;
    background: var(--separator);
  }
</style>
