<script lang="ts">
  import BookOpen from '@lucide/svelte/icons/book-open'
  import CircleUser from '@lucide/svelte/icons/circle-user'
  import Keyboard from '@lucide/svelte/icons/keyboard'
  import Palette from '@lucide/svelte/icons/palette'
  import SettingsIcon from '@lucide/svelte/icons/settings'
  import { onMount, tick } from 'svelte'
  import * as api from './lib/api'
  import { fontStacks } from './lib/fonts'
  import { serviceTitle } from './lib/format'
  import { dragRegion, isMac } from './lib/platform'
  import ShortcutList from './lib/ShortcutList.svelte'
  import { palette } from './lib/theme'
  import type { Account, Font, Settings, Theme } from './lib/types'

  type Pane = 'general' | 'appearance' | 'reading' | 'shortcuts' | 'account'

  const panes: { pane: Pane; label: string; icon: typeof Palette }[] = [
    { pane: 'general', label: 'General', icon: SettingsIcon },
    { pane: 'appearance', label: 'Appearance', icon: Palette },
    { pane: 'reading', label: 'Reading', icon: BookOpen },
    { pane: 'shortcuts', label: 'Shortcuts', icon: Keyboard },
  ]

  const syncIntervals = [
    { minutes: 5, label: 'Every 5 Minutes' },
    { minutes: 15, label: 'Every 15 Minutes' },
    { minutes: 30, label: 'Every 30 Minutes' },
    { minutes: 60, label: 'Every Hour' },
    { minutes: 0, label: 'Manually' },
  ]

  const retention = [
    { days: 7, label: '1 Week' },
    { days: 14, label: '2 Weeks' },
    { days: 30, label: '30 Days' },
    { days: 90, label: '3 Months' },
  ]

  const themes: { value: Theme; label: string }[] = [
    { value: 'system', label: 'System' },
    { value: 'light', label: 'Light' },
    { value: 'dark', label: 'Dark' },
    { value: 'espresso', label: 'Espresso' },
  ]

  const fonts: { font: Font; label: string }[] = [
    { font: 'inter', label: 'Inter' },
    { font: 'system', label: 'System' },
    { font: 'serif', label: 'Serif' },
  ]

  let pane = $state<Pane>('general')
  let settings = $state<Settings | null>(null)
  let account = $state<Account | null>(null)
  let error = $state<string | null>(null)
  let confirmingSignOut = $state(false)
  let signingOut = $state(false)
  let signOutButton = $state<HTMLButtonElement>()
  // Saves run one after another, so they reach config.json in the order made.
  let saving = Promise.resolve()
  // While saves are pending, `settings-changed` only echoes older states of this window.
  let pending = 0

  const current = $derived(
    pane === 'account'
      ? { label: account ? serviceTitle(account.server) : '' }
      : panes.find((p) => p.pane === pane)!,
  )

  onMount(() => {
    void api.onSettingsChanged((s) => {
      if (pending === 0) settings = s
    })
    void api.onSignedIn((a) => (account = a))
    void load()
  })

  async function load() {
    try {
      const session = await api.session()
      settings = session.settings
      account = session.account
    } catch (e) {
      error = api.errorMessage(e)
    }
  }

  $effect(() => {
    if (!settings) return
    document.documentElement.style.fontFamily = fontStacks[settings.uiFont]
    document.documentElement.dataset.theme = palette(settings.theme)
  })

  /** Applies a change at once. The main window gets it through `settings-changed`. A
   * failed save shows the error and goes back to the saved settings. */
  function change(patch: Partial<Settings>) {
    if (!settings) return
    settings = { ...settings, ...patch }
    pending++
    saving = saving.then(async () => {
      try {
        // The state now, not at the change: after a failed save, that went back.
        await api.setSettings($state.snapshot(settings!))
        error = null
      } catch (e) {
        await load()
        error = api.errorMessage(e)
      } finally {
        pending--
      }
    })
  }

  async function signOut() {
    signingOut = true
    try {
      await api.signOut()
    } catch (e) {
      error = api.errorMessage(e)
      signingOut = false
    }
  }

  async function cancelSignOut() {
    confirmingSignOut = false
    await tick()
    signOutButton?.focus()
  }

  // Recording a shortcut takes Escape before it gets here.
  function onkeydown(e: KeyboardEvent) {
    if (e.key !== 'Escape') return
    if (confirmingSignOut) void cancelSignOut()
    else void api.closeWindow()
  }
</script>

<svelte:window {onkeydown} />

<!-- A value that isn't among the options, such as from an edited config.json, shows as
     itself through `other`, not as the first option. -->
{#snippet select<T extends string | number>(
  id: string,
  value: T,
  choices: { value: T; label: string }[],
  onchange: (value: T) => void,
  other: (value: T) => string = String,
)}
  {@const options = choices.some((c) => c.value === value)
    ? choices
    : [{ value, label: other(value) }, ...choices]}
  <select
    {id}
    onchange={(e) => onchange(options[e.currentTarget.selectedIndex].value)}
  >
    {#each options as option (option.value)}
      <option selected={option.value === value}>{option.label}</option>
    {/each}
  </select>
{/snippet}

<div class="settings" class:mac={isMac}>
  <nav aria-label="Settings">
    <div class="drag" data-tauri-drag-region={dragRegion}></div>
    <ul>
      {#each panes as p (p.pane)}
        <li>
          <button
            class="nav-row"
            class:selected={pane === p.pane}
            aria-current={pane === p.pane ? 'page' : undefined}
            onclick={() => (pane = p.pane)}
          >
            <p.icon size={18} strokeWidth={2} />
            {p.label}
          </button>
        </li>
      {/each}
    </ul>
    {#if account}
      <button
        class="nav-row account"
        class:selected={pane === 'account'}
        aria-current={pane === 'account' ? 'page' : undefined}
        onclick={() => (pane = 'account')}
      >
        <CircleUser size={28} strokeWidth={1.5} />
        <span>
          <strong>{serviceTitle(account.server)}</strong>
          <small>{account.username}</small>
        </span>
      </button>
    {/if}
  </nav>

  <main>
    <header data-tauri-drag-region={dragRegion}>
      <h1>{current.label}</h1>
      {#if pane === 'account' && account}<p>{account.username}</p>{/if}
    </header>

    {#if error}<p class="error" role="alert">{error}</p>{/if}

    {#if settings}
      <div class="content">
        {#if pane === 'general'}
          <section>
            <h2>Syncing</h2>
            <div class="group">
              <div class="row">
                <label for="sync">Sync</label>
                {@render select(
                  'sync',
                  settings.syncIntervalMinutes,
                  syncIntervals.map((i) => ({ value: i.minutes, label: i.label })),
                  (syncIntervalMinutes) => change({ syncIntervalMinutes }),
                  (minutes) => `Every ${minutes} Minutes`,
                )}
              </div>
            </div>
          </section>
        {:else if pane === 'appearance'}
          <section>
            <h2>Colors</h2>
            <div class="group">
              <div class="row">
                <label for="theme">Theme</label>
                {@render select('theme', settings.theme, themes, (theme) => change({ theme }))}
              </div>
            </div>
          </section>
          <section>
            <h2>Fonts</h2>
            <div class="group">
              <div class="row">
                <label for="ui-font">Interface</label>
                {@render select(
                  'ui-font',
                  settings.uiFont,
                  fonts.map((f) => ({ value: f.font, label: f.label })),
                  (uiFont) => change({ uiFont }),
                )}
              </div>
              <div class="row">
                <label for="article-font">Articles</label>
                {@render select(
                  'article-font',
                  settings.articleFont,
                  fonts.map((f) => ({ value: f.font, label: f.label })),
                  (articleFont) => change({ articleFont }),
                )}
              </div>
            </div>
          </section>
        {:else if pane === 'reading'}
          <section>
            <h2>Marking as Read</h2>
            <div class="group">
              <div class="row">
                <label for="confirm-mark-all">Ask before marking all as read</label>
                <input
                  id="confirm-mark-all"
                  type="checkbox"
                  role="switch"
                  checked={settings.confirmMarkAllRead}
                  onchange={(e) => change({ confirmMarkAllRead: e.currentTarget.checked })}
                />
              </div>
            </div>
          </section>
        {:else if pane === 'shortcuts'}
          <ShortcutList
            overrides={settings.shortcuts}
            onchange={(shortcuts) => change({ shortcuts })}
          />
        {:else if pane === 'account' && account}
          <section>
            <h2>Account</h2>
            <div class="group">
              <div class="row">
                <span>Server</span>
                <span class="value" title={account.server}>{account.server}</span>
              </div>
              <div class="row">
                <span>Username</span>
                <span class="value">{account.username}</span>
              </div>
            </div>
          </section>
          <section>
            <h2>Storage</h2>
            <div class="group">
              <div class="row">
                <label for="keep-read">Keep read articles</label>
                {@render select(
                  'keep-read',
                  settings.keepReadDays,
                  retention.map((r) => ({ value: r.days, label: r.label })),
                  (keepReadDays) => change({ keepReadDays }),
                  (days) => `${days} Days`,
                )}
              </div>
            </div>
            <p class="note">Starred articles are always kept.</p>
          </section>
          <section>
            {#if confirmingSignOut}
              <div class="group confirm" role="alertdialog" aria-labelledby="sign-out-question">
                <p id="sign-out-question">
                  <strong>Sign out of {serviceTitle(account.server)}?</strong>
                  This removes the account and its articles from this computer. Changes that
                  haven’t synced yet are lost.
                </p>
                <div class="actions">
                  <!-- Cancel takes the focus, so a second Return doesn't sign out. -->
                  <!-- svelte-ignore a11y_autofocus -->
                  <button class="button" autofocus onclick={cancelSignOut}>Cancel</button>
                  <button class="button destructive" disabled={signingOut} onclick={signOut}>
                    Sign Out
                  </button>
                </div>
              </div>
            {:else}
              <button
                class="button"
                bind:this={signOutButton}
                onclick={() => (confirmingSignOut = true)}
              >
                Sign Out…
              </button>
            {/if}
          </section>
        {/if}
      </div>
    {/if}
  </main>
</div>

<style>
  .settings {
    display: flex;
    height: 100%;
  }

  nav {
    display: flex;
    flex-direction: column;
    flex: none;
    width: 200px;
    padding: 0 8px 10px;
    background: var(--sidebar-bg);
  }

  /* Room for the window buttons on macOS, which draws them over the window. */
  .drag {
    flex: none;
    height: 12px;
  }

  .mac .drag {
    height: var(--toolbar-height);
  }

  ul {
    flex: 1;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .nav-row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 32px;
    padding: 0 10px;
    border-radius: 6px;
    text-align: left;
  }

  .nav-row:hover {
    background: var(--hover);
  }

  .nav-row.selected {
    background: var(--accent);
    color: var(--accent-text);
  }

  .nav-row.account {
    height: 48px;
  }

  .account span {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .account strong,
  .account small {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .account small {
    font-size: 12px;
    color: var(--text-2);
  }

  .account.selected small {
    color: inherit;
    opacity: 0.8;
  }

  main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  header {
    flex: none;
    padding: 20px 28px 8px;
  }

  .mac header {
    padding-top: var(--toolbar-height);
  }

  h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 800;
    letter-spacing: -0.015em;
  }

  header p {
    margin: 2px 0 0;
    font-weight: 600;
    color: var(--text-3);
  }

  .content {
    flex: 1;
    overflow-y: auto;
    padding: 8px 28px 28px;
  }

  section + section {
    margin-top: 22px;
  }

  .value {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--text-2);
    user-select: text;
  }

  select {
    font: inherit;
    color: inherit;
    max-width: 60%;
  }

  .error {
    margin: 0 28px;
    color: var(--error);
  }

  .confirm {
    padding: 12px;
  }

  .confirm p {
    margin: 0 0 12px;
    line-height: 1.45;
  }

  .confirm strong {
    display: block;
    margin-bottom: 2px;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  /* A switch, drawn over the checkbox. */
  input[role='switch'] {
    appearance: none;
    flex: none;
    position: relative;
    width: 34px;
    height: 20px;
    margin: 0;
    border-radius: 10px;
    background: var(--active);
    transition: background 120ms ease;
  }

  input[role='switch']::after {
    content: '';
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--thumb-bg);
    box-shadow: 0 0 0 0.5px var(--thumb-edge), 0 1px 2px rgb(0 0 0 / 0.2);
    transition: transform 120ms ease;
  }

  input[role='switch']:checked {
    background: var(--accent);
  }

  input[role='switch']:checked::after {
    transform: translateX(14px);
  }

  @media (prefers-reduced-motion: reduce) {
    input[role='switch'],
    input[role='switch']::after {
      transition: none;
    }
  }
</style>
