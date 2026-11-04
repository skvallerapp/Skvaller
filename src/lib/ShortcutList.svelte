<script lang="ts">
  import { isMac } from './platform'
  import { arrowKeys, bindings, keyLabel, keyName, rebind, shortcutActions } from './shortcuts'
  import type { Settings, ShortcutAction } from './types'

  type Overrides = Settings['shortcuts']

  let { overrides, onchange }: { overrides: Overrides; onchange: (overrides: Overrides) => void } =
    $props()

  const keys = $derived(bindings(overrides))
  const labelOf = (action: ShortcutAction) =>
    shortcutActions.find((a) => a.action === action)!.label

  /** The action waiting for its new key. */
  let recordingAction = $state<ShortcutAction | null>(null)
  let notice = $state<string | null>(null)

  function start(action: ShortcutAction) {
    recordingAction = recordingAction === action ? null : action
    notice = null
  }

  function assign(key: string | null) {
    const action = recordingAction!
    recordingAction = null
    const result = rebind(overrides, action, key)
    notice = result.displaced
      ? `${keyLabel(key!, isMac)} was the key for ${labelOf(result.displaced)}, which now has none.`
      : null
    onchange(result.overrides)
  }

  // Captures the key before anything else sees it, so a key like Space or Escape
  // doesn't also press the focused button or close the window.
  function onkeydown(e: KeyboardEvent) {
    if (!recordingAction) return
    if (e.key === 'Tab') {
      recordingAction = null
      return
    }
    e.preventDefault()
    e.stopPropagation()
    if (e.key === 'Escape') {
      recordingAction = null
      notice = null
      return
    }
    if (e.key === 'Backspace' || e.key === 'Delete') {
      assign(null)
      return
    }
    const key = keyName(e)
    if (key === null) {
      // A modifier on its own: wait for the rest. Cmd, Ctrl or Alt combinations: refuse.
      if (e.metaKey || e.ctrlKey || e.altKey) notice = 'A shortcut is a single key, with or without Shift.'
      return
    }
    if (arrowKeys.has(key)) {
      notice = 'The arrow keys move in and between the sidebar and the article list.'
      return
    }
    // Enter has to keep pressing the focused button.
    if (key === 'Enter' || key === 'Shift+Enter') {
      notice = `${keyLabel('Enter', isMac)} can’t be a shortcut.`
      return
    }
    assign(key)
  }

  // On macOS a click doesn't focus a button, so its blur can't end recording.
  function onpointerdown(e: PointerEvent) {
    if (recordingAction && !(e.target as Element).closest?.('.key.recording')) recordingAction = null
  }

  function restoreDefaults() {
    recordingAction = null
    notice = null
    onchange({})
  }
</script>

<svelte:window onkeydowncapture={onkeydown} {onpointerdown} />

<section>
  <h2>Keys</h2>
  <div class="group">
    {#each shortcutActions as { action, label } (action)}
      <div class="row">
        <span id="shortcut-{action}">{label}</span>
        <button
          class="key"
          class:recording={recordingAction === action}
          class:none={keys[action] === null}
          aria-labelledby="shortcut-{action} shortcut-{action}-key"
          onclick={() => start(action)}
          onblur={() => recordingAction === action && (recordingAction = null)}
        >
          <span id="shortcut-{action}-key">
            {#if recordingAction === action}
              Press a key…
            {:else if keys[action] === null}
              None
            {:else}
              {keyLabel(keys[action]!, isMac)}
            {/if}
          </span>
        </button>
      </div>
    {/each}
  </div>
  <p class="note" aria-live="polite">
    {#if notice}
      {notice}
    {:else if recordingAction}
      Press Esc to cancel, or {isMac ? 'Delete' : 'Backspace'} for no key.
    {:else}
      Click a key to change it. ↑ and ↓ move in the sidebar or the list, and ← and → move between them.
    {/if}
  </p>
  <button class="button restore" disabled={Object.keys(overrides).length === 0} onclick={restoreDefaults}>
    Restore Defaults
  </button>
</section>

<style>
  .key {
    min-width: 64px;
    height: 26px;
    padding: 0 10px;
    border-radius: 6px;
    border: 1px solid var(--separator);
    background: var(--reader-bg);
    font-weight: 600;
    text-align: center;
  }

  .key.none {
    color: var(--text-3);
  }

  .key.recording {
    border-color: var(--accent);
    color: var(--accent);
  }

  .note {
    min-height: 1.4em;
    margin-bottom: 12px;
  }

  .restore:disabled {
    opacity: 0.5;
  }
</style>
