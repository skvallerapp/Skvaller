<script lang="ts">
  import { t } from './i18n.svelte'
  import { isMac } from './platform'
  import { arrowKeys, bindings, keyLabel, keyName, rebind, shortcutActions } from './shortcuts'
  import type { Settings, ShortcutAction } from './types'

  type Overrides = Settings['shortcuts']

  let { overrides, onchange }: { overrides: Overrides; onchange: (overrides: Overrides) => void } =
    $props()

  const keys = $derived(bindings(overrides))
  const labelOf = (action: ShortcutAction) => t(`shortcuts.actions.${action}`)

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
      ? t('shortcuts.displaced', { key: keyLabel(key!, isMac), action: labelOf(result.displaced) })
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
      if (e.metaKey || e.ctrlKey || e.altKey) notice = t('shortcuts.singleKey')
      return
    }
    if (arrowKeys.has(key)) {
      notice = t('shortcuts.arrows')
      return
    }
    // Enter has to keep pressing the focused button.
    if (key === 'Enter' || key === 'Shift+Enter') {
      notice = t('shortcuts.notAllowed', { key: keyLabel('Enter', isMac) })
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
  <h2>{t('shortcuts.heading')}</h2>
  <div class="group">
    {#each shortcutActions as action (action)}
      <div class="row">
        <span id="shortcut-{action}">{labelOf(action)}</span>
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
              {t('shortcuts.pressKey')}
            {:else if keys[action] === null}
              {t('shortcuts.none')}
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
      {t('shortcuts.recording', { key: t(isMac ? 'keys.delete' : 'keys.backspace') })}
    {:else}
      {t('shortcuts.help')}
    {/if}
  </p>
  <button class="button restore" disabled={Object.keys(overrides).length === 0} onclick={restoreDefaults}>
    {t('shortcuts.restore')}
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
