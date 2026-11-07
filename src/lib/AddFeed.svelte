<script lang="ts">
  import Plus from '@lucide/svelte/icons/plus'
  import { tick } from 'svelte'
  import { errorMessage } from './api'
  import { sentence } from './format'
  import { t } from './i18n.svelte'
  import { store } from './store.svelte'
  import type { CommandError } from './types'

  /** The folder select's value for a folder made on the spot. */
  const NEW_FOLDER = '\u0000new'

  // Closes on Escape, Cancel, adding, or a click elsewhere, like the account menu.
  let open = $state(false)
  let root: HTMLElement
  let trigger: HTMLButtonElement
  let urlInput = $state<HTMLInputElement>()
  let url = $state('')
  let folder = $state('')
  let newFolder = $state('')
  let busy = $state(false)
  let error = $state<string | null>(null)

  const messages = $derived<Partial<Record<CommandError['kind'], string>>>({
    noFeed: t('addFeed.noFeed'),
    badUrl: t('addFeed.badUrl'),
    network: t('addFeed.network'),
  })

  async function toggle() {
    open = !open
    if (!open) return
    error = null
    // The selected folder, if any, is where a new feed most likely belongs.
    folder = store.scope.kind === 'folder' ? store.scope.id : ''
    await tick()
    urlInput?.focus()
  }

  function close() {
    if (busy) return
    open = false
    trigger.focus()
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault()
    busy = true
    error = null
    try {
      const folderId = folder === NEW_FOLDER ? await store.createFolder(newFolder) : folder || null
      await store.subscribe(url, folderId)
      url = ''
      newFolder = ''
      busy = false
      close()
    } catch (err) {
      // Tauri's own errors are plain strings.
      error = messages[(err as CommandError | null)?.kind ?? 'other'] ?? sentence(errorMessage(err))
    } finally {
      busy = false
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      close()
    }
  }

  // Only focus moving to another element closes it; WebKit doesn't focus clicked
  // buttons, so clicks outside close it in onWindowPointerdown.
  function onFocusout(e: FocusEvent) {
    const next = e.relatedTarget as Node | null
    if (next && !busy && !root.contains(next)) open = false
  }

  function onWindowPointerdown(e: PointerEvent) {
    if (open && !busy && !root.contains(e.target as Node)) open = false
  }
</script>

<svelte:window onpointerdown={onWindowPointerdown} />

<div bind:this={root} onfocusout={onFocusout}>
  <button
    class="icon-button"
    bind:this={trigger}
    aria-label={t('addFeed.title')}
    title={t('addFeed.title')}
    aria-haspopup="dialog"
    aria-expanded={open}
    onclick={toggle}
  >
    <Plus size={18} strokeWidth={2} />
  </button>
  {#if open}
    <div class="popover" role="dialog" aria-label={t('addFeed.title')} tabindex="-1" data-tauri-drag-region="false" onkeydown={onKeydown}>
    <form onsubmit={submit}>
      <label>
        <span>{t('addFeed.address')}</span>
        <input
          bind:this={urlInput}
          bind:value={url}
          placeholder="example.com/feed.xml"
          autocapitalize="off"
          autocorrect="off"
          spellcheck="false"
          required
        />
      </label>
      <label>
        <span>{t('addFeed.folder')}</span>
        <select bind:value={folder}>
          <option value="">{t('addFeed.noFolder')}</option>
          {#each store.folders as f (f.id)}<option value={f.id}>{f.title}</option>{/each}
          <option value={NEW_FOLDER}>{t('addFeed.newFolderOption')}</option>
        </select>
      </label>
      {#if folder === NEW_FOLDER}
        <label>
          <span>{t('addFeed.newFolder')}</span>
          <input bind:value={newFolder} required />
        </label>
      {/if}
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <div class="actions">
        <button type="button" class="button" onclick={close}>{t('common.cancel')}</button>
        <button class="button primary" disabled={busy}>{busy ? t('common.adding') : t('addFeed.add')}</button>
      </div>
    </form>
    </div>
  {/if}
</div>

<style>
  /* Spans the sidebar, like the account menu. */
  .popover {
    position: absolute;
    top: calc(100% - 4px);
    left: 8px;
    right: 8px;
    z-index: 10;
    padding: 12px;
    border: 1px solid var(--separator);
    border-radius: 10px;
    background: var(--reader-bg);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.18);
    animation: appear 140ms ease-out;
    transform-origin: top;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
  }

  input,
  select {
    height: 28px;
    padding: 0 8px;
    border: 1px solid var(--separator);
    border-radius: 6px;
    background: var(--list-bg);
    color: var(--text);
    font: inherit;
    font-size: 13px;
    font-weight: 400;
  }

  input:focus,
  select:focus {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
  }

  .error {
    margin: 0;
    font-size: 12px;
    color: var(--error);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .button:disabled {
    opacity: 0.6;
  }
</style>
