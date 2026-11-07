<script lang="ts">
  import BookOpen from '@lucide/svelte/icons/book-open'
  import ChevronsUpDown from '@lucide/svelte/icons/chevrons-up-down'
  import CircleUser from '@lucide/svelte/icons/circle-user'
  import Keyboard from '@lucide/svelte/icons/keyboard'
  import Palette from '@lucide/svelte/icons/palette'
  import ChevronLeft from '@lucide/svelte/icons/chevron-left'
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import Plus from '@lucide/svelte/icons/plus'
  import Rss from '@lucide/svelte/icons/rss'
  import { onMount, tick } from 'svelte'
  import * as api from './lib/api'
  import { sentence } from './lib/format'
  import { slideHighlight } from './lib/highlight.svelte'
  import { languageName, languages, t, type MessageKey, type ParamsFor } from './lib/i18n.svelte'
  import { dragRegion, macWindow } from './lib/platform'
  import ShortcutList from './lib/ShortcutList.svelte'
  import AddAccount from './lib/AddAccount.svelte'
  import { styleRoot } from './lib/theme'
  import type { Account, AccountChange, Feed, Font, Session, Settings, SyncSchedule, Theme } from './lib/types'

  /** A section of the window. An account's is `account:<id>`. */
  type Pane = 'appearance' | 'reading' | 'shortcuts' | 'add-account' | `account:${string}`

  /** A choice whose label is the message for `key`, in the UI's language. */
  type Choice<T> = { value: T; readonly label: string }
  const choice = <T, K extends MessageKey>(value: T, key: K, ...params: ParamsFor<K>): Choice<T> => ({
    value,
    get label() {
      return t(key, ...params)
    },
  })

  const section = (pane: Pane, key: MessageKey, icon: typeof Palette) => ({
    pane,
    icon,
    get label() {
      return t(key)
    },
  })

  const panes = [
    section('appearance', 'settings.appearance', Palette),
    section('reading', 'settings.reading', BookOpen),
    section('shortcuts', 'settings.shortcuts', Keyboard),
  ]

  const syncSchedules: Choice<SyncSchedule>[] = [
    choice('onStart', 'settings.syncSchedules.onStart'),
    choice(5, 'settings.syncSchedules.everyMinutes', { count: 5 }),
    choice(15, 'settings.syncSchedules.everyMinutes', { count: 15 }),
    choice(30, 'settings.syncSchedules.everyMinutes', { count: 30 }),
    choice(60, 'settings.syncSchedules.everyHour'),
    choice('manual', 'settings.syncSchedules.manual'),
  ]

  const retention: Choice<number>[] = [
    choice(7, 'settings.weeks', { count: 1 }),
    choice(14, 'settings.weeks', { count: 2 }),
    choice(30, 'settings.months', { count: 1 }),
    choice(90, 'settings.months', { count: 3 }),
  ]

  // 0 keeps them.
  const unreadRetention: Choice<number>[] = [
    choice(30, 'settings.months', { count: 1 }),
    choice(90, 'settings.months', { count: 3 }),
    choice(180, 'settings.months', { count: 6 }),
    choice(365, 'settings.years', { count: 1 }),
    choice(0, 'settings.keepForever'),
  ]

  const sortOrders: Choice<Settings['sortSubscriptions']>[] = [
    choice('foldersFirst', 'settings.sortOrders.foldersFirst'),
    choice('alphabetical', 'settings.sortOrders.alphabetical'),
  ]

  const markReadModes: Choice<Settings['markReadOn']>[] = [
    choice('open', 'settings.markReadModes.open'),
    choice('delay', 'settings.markReadModes.delay'),
    choice('manual', 'settings.markReadModes.manual'),
  ]

  const thumbnailSizes: Choice<Settings['thumbnailSize']>[] = [
    choice('none', 'settings.thumbnailSizes.none'),
    choice('small', 'settings.thumbnailSizes.small'),
    choice('medium', 'settings.thumbnailSizes.medium'),
    choice('large', 'settings.thumbnailSizes.large'),
  ]

  const themes: Choice<Theme>[] = [
    choice('system', 'settings.themes.system'),
    choice('light', 'settings.themes.light'),
    choice('dark', 'settings.themes.dark'),
    choice('espresso', 'settings.themes.espresso'),
  ]

  // The bundled fonts go by their names, in every language.
  const fonts: Choice<Font>[] = [
    { value: 'inter', label: 'Inter' },
    { value: 'geist', label: 'Geist' },
    choice('system', 'settings.fontNames.system'),
    { value: 'source-serif', label: 'Source Serif' },
    { value: 'literata', label: 'Literata' },
    choice('serif', 'settings.fontNames.serif'),
  ]

  /** The language setting's choices: the system's, then each language by its own name.
   * Only shown once there is more than English. */
  const languageChoices: Choice<string>[] = [
    choice('', 'settings.languageSystem'),
    ...languages.map((tag) => ({ value: tag, label: languageName(tag) })),
  ]

  let pane = $state<Pane>('appearance')
  let settings = $state<Settings | null>(null)
  let accounts = $state.raw<Account[]>([])
  let error = $state<string | null>(null)
  let confirmingRemove = $state(false)
  let removing = $state(false)
  let removeButton = $state<HTMLButtonElement>()
  /** What the last OPML import or export did, for the account it was for. */
  let opmlNote = $state<{ id: string; text: string } | null>(null)
  // Saves run one after another, so they reach config.json in the order made.
  let saving = Promise.resolve()
  // While saves are pending, `settings-changed` only echoes older states of this window.
  let pending = 0

  /** A page inside the account's pane: its feeds' Reader View settings. */
  let subpage = $state<'reader-view' | null>(null)
  let addAccountVisits = $state(0)
  /** The account's subscribed feeds, for the Reader View page. */
  let feeds = $state.raw<Feed[] | null>(null)
  let feedsFor = ''
  /** Reader View changes this window is saving: their echoes would show older states. */
  let feedSaves = 0

  const paneOf = (a: Account): Pane => `account:${a.id}`
  /** The account whose pane shows. */
  const account = $derived(accounts.find((a) => paneOf(a) === pane) ?? null)
  const current = $derived(
    account && subpage === 'reader-view'
      ? { label: t('settings.readerView') }
      : account
      ? { label: account.description }
      : pane === 'add-account'
        ? { label: t('addAccount.title') }
        : (panes.find((p) => p.pane === pane) ?? panes[0]),
  )

  onMount(() => {
    void api.onSettingsChanged((s) => {
      if (pending === 0) settings = s
    })
    void api.onAccountsChanged((session) => {
      // While changes are saving, an echo of an older one would undo the newer. A
      // removed account still goes.
      if (session.revision < revision) return
      revision = session.revision
      if (accountSaves === 0 || session.accounts.length !== accounts.length) setAccounts(session.accounts)
    })
    void api.onSettingsPane((p) => show(p as Pane))
    void api.onFeedsChanged((id) => {
      if (subpage === 'reader-view' && id === feedsFor && feedSaves === 0) void loadFeeds(id)
    })
    void load().then(async () => {
      const asked = await api.takeSettingsPane()
      if (asked) show(asked as Pane)
    })
  })

  async function load() {
    try {
      const session = await api.session()
      settings = session.settings
      setAccounts(session.accounts)
    } catch (e) {
      error = sentence(api.errorMessage(e))
    }
  }

  /** A removed account's pane gives way to the next account's, or the first section. */
  function setAccounts(next: Account[]) {
    const removed = pane.startsWith('account:') && !next.some((a) => paneOf(a) === pane)
    accounts = next
    if (removed) {
      subpage = null
      confirmingRemove = false
      removing = false
      pane = next[0] ? paneOf(next[0]) : panes[0].pane
    }
  }

  function show(p: Pane) {
    // Add Account again: back to the list from a service's form.
    if (p === 'add-account') addAccountVisits++
    confirmingRemove = false
    subpage = null
    opmlNote = null
    pane = p
  }

  $effect(() => {
    if (settings) styleRoot(settings)
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
        error = sentence(api.errorMessage(e))
      } finally {
        pending--
      }
    })
  }

  /** Applies a change to an account at once. Every window gets it through
   * `accounts-changed`. A failed save shows the error and goes back. */
  let accountSaves = 0
  /** The newest session taken in. */
  let revision = -1

  function changeAccount(id: string, change: AccountChange) {
    accounts = accounts.map((a) => (a.id === id ? { ...a, ...change } : a))
    accountSaves++
    api
      .updateAccount(id, change)
      .then(
        () => (error = null),
        (e) => (error = sentence(api.errorMessage(e))),
      )
      .finally(async () => {
        // The last save reloads the accounts, so the form shows what was saved.
        if (--accountSaves === 0) {
          const saved = await api.accounts().catch(() => null)
          if (saved && accountSaves === 0) setAccounts(saved)
        }
      })
  }

  function renameAccount(input: HTMLInputElement, a: Account) {
    const description = input.value.trim()
    if (description && description !== a.description) changeAccount(a.id, { description })
    else input.value = a.description
  }

  async function importOpml(id: string) {
    try {
      const report = await api.importOpml(id)
      if (!report) return
      const text =
        report.added === 0
          ? t('settings.importedNone')
          : report.skipped === 0
            ? t('settings.imported', { count: report.added })
            : t('common.twoSentences', {
                first: t('settings.imported', { count: report.added }),
                second: t('settings.importSkipped', { count: report.skipped }),
              })
      opmlNote = { id, text }
      error = null
    } catch (e) {
      error = sentence(api.errorMessage(e))
    }
  }

  async function exportOpml(id: string) {
    try {
      if (await api.exportOpml(id)) opmlNote = { id, text: t('settings.exported') }
      error = null
    } catch (e) {
      error = sentence(api.errorMessage(e))
    }
  }

  async function openReaderView(id: string) {
    subpage = 'reader-view'
    feeds = null
    // The row that opened the page goes: focus moves to the way back.
    await tick()
    main.querySelector<HTMLElement>('.back')?.focus()
    await loadFeeds(id)
  }

  let feedsRequest = 0

  async function loadFeeds(id: string) {
    feedsFor = id
    const request = ++feedsRequest
    try {
      const library = await api.library(id)
      // Only the newest load, for the page still showing.
      if (request === feedsRequest && subpage === 'reader-view' && feedsFor === id)
        feeds = library.feeds.filter((f) => f.subscribed)
    } catch (e) {
      if (request === feedsRequest && subpage === 'reader-view') error = sentence(api.errorMessage(e))
    }
  }

  function setReaderView(id: string, feed: Feed, on: boolean) {
    feeds = (feeds ?? []).map((f) => (f.id === feed.id ? { ...f, readerView: on } : f))
    feedSaves++
    api
      .setFeedReaderView(id, feed.id, on)
      .then(
        () => (error = null),
        (e) => (error = sentence(api.errorMessage(e))),
      )
      .finally(() => {
        // The last save reloads, so the page shows what was saved.
        if (--feedSaves === 0 && subpage === 'reader-view' && feedsFor === id) void loadFeeds(id)
      })
  }

  async function backToAccount() {
    subpage = null
    await tick()
    main.querySelector<HTMLElement>('#reader-view-row')?.focus()
  }

  async function removeAccount(id: string) {
    removing = true
    try {
      await api.removeAccount(id)
    } catch (e) {
      error = sentence(api.errorMessage(e))
    } finally {
      removing = false
    }
  }

  async function cancelRemove() {
    confirmingRemove = false
    await tick()
    removeButton?.focus()
  }

  async function addAccount(server: string, username: string, password: string) {
    showAdded(await api.signIn(server, username, password))
  }

  async function addLocalAccount() {
    showAdded(await api.addLocalAccount())
  }

  function showAdded(session: Session) {
    setAccounts(session.accounts)
    const added = session.accounts.find((a) => a.id === session.activeId)
    if (added) show(paneOf(added))
  }

  let nav: HTMLElement
  let main: HTMLElement

  slideHighlight(() => nav, () => pane)

  /** The sections in the order they're listed. */
  const order = $derived<Pane[]>([...panes.map((p) => p.pane), ...accounts.map(paneOf), 'add-account'])

  /** Selects the section above or below. A focused section row passes the focus on. */
  async function stepPane(delta: 1 | -1) {
    const next = order[order.indexOf(pane) + delta]
    if (!next) return
    const hadFocus = nav.contains(document.activeElement)
    show(next)
    await tick()
    if (hadFocus) nav.querySelector<HTMLElement>('.selected')?.focus({ preventScroll: true })
  }

  // Recording a shortcut takes Escape before it gets here.
  function onkeydown(e: KeyboardEvent) {
    // ↑ and ↓ switch sections, unless the focus is in the form, whose controls use them.
    const plain = !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey
    if (plain && (e.key === 'ArrowDown' || e.key === 'ArrowUp') && !main.contains(e.target as Node)) {
      e.preventDefault()
      void stepPane(e.key === 'ArrowDown' ? 1 : -1)
      return
    }
    if (e.key !== 'Escape') return
    // In a text field, Escape leaves it instead of closing the window, and undoes the
    // typing in Description.
    const target = e.target as HTMLElement
    if (target instanceof HTMLInputElement && target.type !== 'checkbox') {
      if (target.id === 'description' && account) target.value = account.description
      target.blur()
      return
    }
    if (subpage) void backToAccount()
    else if (confirmingRemove) void cancelRemove()
    else void api.closeWindow()
  }
</script>

<svelte:window {onkeydown} />

<!-- A value that isn't among the options, such as from an edited config.json, shows as
     itself through `other`, not as the first option. -->
{#snippet select<T extends string | number>(
  id: string,
  value: T,
  choices: { value: T; readonly label: string }[],
  onchange: (value: T) => void,
  other: (value: T) => string = String,
)}
  {@const options = choices.some((c) => c.value === value)
    ? choices
    : [{ value, label: other(value) }, ...choices]}
  <span class="select">
    <select
      {id}
      onchange={(e) => onchange(options[e.currentTarget.selectedIndex].value)}
    >
      {#each options as option (option.value)}
        <option selected={option.value === value}>{option.label}</option>
      {/each}
    </select>
    <ChevronsUpDown size={14} strokeWidth={2} aria-hidden="true" />
  </span>
{/snippet}

<div class="settings" class:mac={macWindow}>
  <nav aria-label={t('settings.label')} bind:this={nav}>
    <div class="drag" data-tauri-drag-region={dragRegion}></div>
    <ul>
      {#each panes as p (p.pane)}
        <li>
          <button
            class="nav-row"
            class:selected={pane === p.pane}
            aria-current={pane === p.pane ? 'page' : undefined}
            onclick={() => show(p.pane)}
          >
            <span class="highlight"></span>
            <p.icon size={18} strokeWidth={2} />
            {p.label}
          </button>
        </li>
      {/each}
    </ul>
    <ul class="accounts" aria-label={t('settings.accounts')}>
      {#each accounts as a (a.id)}
        <li>
          <button
            class="nav-row account"
            class:selected={pane === paneOf(a)}
            aria-current={pane === paneOf(a) ? 'page' : undefined}
            onclick={() => show(paneOf(a))}
          >
            <span class="highlight"></span>
            <CircleUser size={28} strokeWidth={1.5} />
            <span>
              <strong>{a.description}</strong>
              <small>{a.kind === 'greader' ? a.username : t('settings.localAccount')}</small>
            </span>
          </button>
        </li>
      {/each}
      <li>
        <button
          class="nav-row"
          class:selected={pane === 'add-account'}
          aria-current={pane === 'add-account' ? 'page' : undefined}
          onclick={() => show('add-account')}
        >
          <span class="highlight"></span>
          <Plus size={18} strokeWidth={2} />
          {t('accounts.add')}
        </button>
      </li>
    </ul>
  </nav>

  <main bind:this={main}>
    <header data-tauri-drag-region={dragRegion}>
      {#if account && subpage}
        <button class="back" aria-label={t('settings.back', { account: account.description })} onclick={backToAccount}>
          <ChevronLeft size={16} strokeWidth={2} />{account.description}
        </button>
      {/if}
      <h1>{current.label}</h1>
      {#if account?.kind === 'greader' && !subpage}<p>{account.username}</p>{/if}
    </header>

    {#if error}<p class="error" role="alert">{error}</p>{/if}

    {#if settings}
      <div class="content">
        {#if pane === 'appearance'}
          {#if languages.length > 1}
            <section>
              <h2>{t('settings.language')}</h2>
              <div class="group">
                <div class="row">
                  <label for="language">{t('settings.language')}</label>
                  {@render select('language', settings.language ?? '', languageChoices, (language) =>
                    change({ language: language || null }),
                  )}
                </div>
              </div>
            </section>
          {/if}
          <section>
            <h2>{t('settings.colors')}</h2>
            <div class="group">
              <div class="row">
                <label for="theme">{t('settings.theme')}</label>
                {@render select('theme', settings.theme, themes, (theme) => change({ theme }))}
              </div>
            </div>
          </section>
          <section>
            <h2>{t('settings.articleList')}</h2>
            <div class="group">
              <div class="row">
                <label for="thumbnails">{t('settings.thumbnails')}</label>
                {@render select('thumbnails', settings.thumbnailSize, thumbnailSizes, (thumbnailSize) =>
                  change({ thumbnailSize }),
                )}
              </div>
            </div>
            <p class="note">{t('settings.thumbnailsNote')}</p>
          </section>
          <section>
              <h2>{t('settings.appIcon')}</h2>
              <div class="group">
                <div class="row">
                  <label for="badge">{t('settings.badge')}</label>
                  <input
                    id="badge"
                    type="checkbox"
                    role="switch"
                    checked={settings.unreadBadge}
                    onchange={(e) => change({ unreadBadge: e.currentTarget.checked })}
                  />
                </div>
              </div>
              <p class="note">{t('settings.badgeNote')}</p>
            </section>
          <section>
            <h2>{t('settings.motion')}</h2>
            <div class="group">
              <div class="row">
                <label for="animations">{t('settings.animations')}</label>
                <input
                  id="animations"
                  type="checkbox"
                  role="switch"
                  checked={settings.animations}
                  onchange={(e) => change({ animations: e.currentTarget.checked })}
                />
              </div>
            </div>
            <p class="note">{t('settings.animationsNote')}</p>
          </section>
          <section>
            <h2>{t('settings.fonts')}</h2>
            <div class="group">
              <div class="row">
                <label for="ui-font">{t('settings.uiFont')}</label>
                {@render select(
                  'ui-font',
                  settings.uiFont,
                  fonts,
                  (uiFont) => change({ uiFont }),
                )}
              </div>
              <div class="row">
                <label for="headline-font">{t('settings.headlineFont')}</label>
                {@render select(
                  'headline-font',
                  settings.headlineFont,
                  fonts,
                  (headlineFont) => change({ headlineFont }),
                )}
              </div>
              <div class="row">
                <label for="article-font">{t('settings.articleFont')}</label>
                {@render select(
                  'article-font',
                  settings.articleFont,
                  fonts,
                  (articleFont) => change({ articleFont }),
                )}
              </div>
            </div>
          </section>
        {:else if pane === 'reading'}
          <section>
            <h2>{t('settings.subscriptions')}</h2>
            <div class="group">
              <div class="row">
                <label for="sort">{t('settings.sort')}</label>
                {@render select('sort', settings.sortSubscriptions, sortOrders, (sortSubscriptions) =>
                  change({ sortSubscriptions }),
                )}
              </div>
            </div>
          </section>
          <section>
            <h2>{t('settings.markingRead')}</h2>
            <div class="group">
              <div class="row">
                <label for="mark-read-on">{t('settings.markReadOn')}</label>
                {@render select('mark-read-on', settings.markReadOn, markReadModes, (markReadOn) =>
                  change({ markReadOn }),
                )}
              </div>
              <div class="row">
                <label for="mark-on-scroll">{t('settings.markOnScroll')}</label>
                <input
                  id="mark-on-scroll"
                  type="checkbox"
                  role="switch"
                  checked={settings.markReadOnScroll}
                  onchange={(e) => change({ markReadOnScroll: e.currentTarget.checked })}
                />
              </div>
              <div class="row">
                <label for="confirm-mark-all">{t('settings.confirmMarkAll')}</label>
                <input
                  id="confirm-mark-all"
                  type="checkbox"
                  role="switch"
                  checked={settings.confirmMarkAllRead}
                  onchange={(e) => change({ confirmMarkAllRead: e.currentTarget.checked })}
                />
              </div>
            </div>
            <p class="note">{t('settings.markReadNote')}</p>
          </section>
        {:else if pane === 'shortcuts'}
          <ShortcutList
            overrides={settings.shortcuts}
            onchange={(shortcuts) => change({ shortcuts })}
          />
        {:else if pane === 'add-account'}
          <section>
            {#key addAccountVisits}
              <AddAccount
                embedded
                submit={addAccount}
                addLocal={accounts.some((a) => a.kind === 'local') ? null : addLocalAccount}
              />
            {/key}
          </section>
        {:else if account && subpage === 'reader-view'}
          {@const a = account}
          <section>
            {#if feeds?.length === 0}
              <p class="intro">{t('settings.noSubscriptions')}</p>
            {:else if feeds}
              <div class="group">
                {#each feeds as feed (feed.id)}
                  <div class="row">
                    <label class="feed" for="rv-{feed.id}">
                      <span class="thumb">
                        {#if feed.icon}<img src={feed.icon} alt="" />{:else}<Rss size={12} strokeWidth={2.5} />{/if}
                      </span>
                      <span class="feed-title">{feed.title}</span>
                    </label>
                    <input
                      id="rv-{feed.id}"
                      type="checkbox"
                      role="switch"
                      checked={feed.readerView}
                      onchange={(e) => setReaderView(a.id, feed, e.currentTarget.checked)}
                    />
                  </div>
                {/each}
              </div>
            {/if}
            <p class="note">{t('settings.readerViewNote')}</p>
          </section>
        {:else if account}
          {@const a = account}
          <section>
            <h2>{t('settings.general')}</h2>
            <div class="group">
              <div class="row">
                <label for="description">{t('settings.accountName')}</label>
                <input
                  id="description"
                  class="text"
                  value={a.description}
                  autocomplete="off"
                  spellcheck="false"
                  onchange={(e) => renameAccount(e.currentTarget, a)}
                  onkeydown={(e) => {
                    if (e.key === 'Enter') e.currentTarget.blur()
                  }}
                />
              </div>
              {#if a.kind === 'greader'}
                <div class="row">
                  <span>{t('settings.server')}</span>
                  <span class="value" title={a.server}>{a.server}</span>
                </div>
              {/if}
            </div>
          </section>
          <section>
            <h2>{t('settings.reading')}</h2>
            <div class="group">
              <button id="reader-view-row" class="row link" onclick={() => openReaderView(a.id)}>
                <span>{t('settings.readerViewRow')}</span>
                <ChevronRight size={16} strokeWidth={2} aria-hidden="true" />
              </button>
            </div>
          </section>
          <section>
            <h2>{t('settings.syncing')}</h2>
            <div class="group">
              <div class="row">
                <label for="sync">{t('settings.syncSchedule')}</label>
                {@render select(
                  'sync',
                  a.sync,
                  syncSchedules,
                  (sync) => changeAccount(a.id, { sync }),
                  (minutes) => t('settings.syncSchedules.everyMinutes', { count: Number(minutes) }),
                )}
              </div>
              <div class="row">
                <label for="sync-on-wake">{t('settings.syncOnWake')}</label>
                <input
                  id="sync-on-wake"
                  type="checkbox"
                  role="switch"
                  checked={a.syncOnWake}
                  onchange={(e) => changeAccount(a.id, { syncOnWake: e.currentTarget.checked })}
                />
              </div>
              <div class="row">
                <label for="keep-read">{t('settings.keepRead')}</label>
                {@render select(
                  'keep-read',
                  a.keepReadDays,
                  retention,
                  (keepReadDays) => changeAccount(a.id, { keepReadDays }),
                  (days) => t('settings.days', { count: days }),
                )}
              </div>
              {#if a.kind === 'local'}
                <div class="row">
                  <label for="keep-unread">{t('settings.keepUnread')}</label>
                  {@render select(
                    'keep-unread',
                    a.keepUnreadDays,
                    unreadRetention,
                    (keepUnreadDays) => changeAccount(a.id, { keepUnreadDays }),
                    (days) => t('settings.days', { count: days }),
                  )}
                </div>
              {/if}
            </div>
            <p class="note">{t('settings.keepStarredNote')}</p>
          </section>
          <section>
            <h2>{t('settings.subscriptions')}</h2>
            <div class="buttons">
              {#if a.kind === 'local'}
                <button class="button wide" onclick={() => importOpml(a.id)}>{t('settings.importOpml')}</button>
              {/if}
              <button class="button wide" onclick={() => exportOpml(a.id)}>{t('settings.exportOpml')}</button>
            </div>
            {#if opmlNote?.id === a.id}<p class="note" role="status">{opmlNote.text}</p>{/if}
          </section>
          <section>
            <h2>{t('settings.account')}</h2>
            {#if confirmingRemove}
              <div class="group confirm" role="alertdialog" aria-labelledby="remove-question">
                <p id="remove-question">
                  <strong>{t('settings.removeQuestion', { account: a.description })}</strong>
                  {a.kind === 'local' ? t('settings.removeLocal') : t('settings.removeSync')}
                </p>
                <div class="actions">
                  <!-- Cancel takes the focus, so a second Return doesn't remove it. -->
                  {#if a.kind === 'local'}
                    <button class="button export" onclick={() => exportOpml(a.id)}>{t('settings.exportOpml')}</button>
                  {/if}
                  <!-- svelte-ignore a11y_autofocus -->
                  <button class="button" autofocus onclick={cancelRemove}>{t('common.cancel')}</button>
                  <button
                    class="button destructive"
                    disabled={removing}
                    onclick={() => removeAccount(a.id)}
                  >
                    {t('settings.remove')}
                  </button>
                </div>
              </div>
            {:else}
              <button
                class="button wide remove"
                bind:this={removeButton}
                onclick={() => (confirmingRemove = true)}
              >
                {t('settings.removeAccount')}
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
    /* The row highlights' stacking context: under every row's text, over the background. */
    isolation: isolate;
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
    margin: 0;
    padding: 0;
    list-style: none;
  }

  ul:first-of-type {
    flex: 1;
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
    position: relative;
    transition: color 120ms ease-out;
  }

  .nav-row:hover {
    background: var(--hover);
  }

  /* The selection's background, behind every row's text (see nav), so it can slide past
     other rows. */
  .highlight {
    position: absolute;
    inset: 0;
    z-index: -1;
    border-radius: inherit;
  }

  .nav-row.selected {
    color: var(--accent-text);
  }

  .nav-row.selected .highlight {
    background: var(--accent);
  }

  /* While the form has the focus, the section is muted, as a pane out of focus is in the
     main window. The tint lowers the contrast of dimmer text, so it takes the full color. */
  .settings:has(main:focus-within) .nav-row.selected {
    color: var(--text);
  }

  .settings:has(main:focus-within) .nav-row.selected .highlight {
    background: var(--accent-muted);
  }

  /* The highlight shows the focused selection; a ring would show ahead of its slide.
     Forced colors (High Contrast) override the color, so the ring shows there. */
  .nav-row.selected:focus-visible {
    outline-color: transparent;
  }

  /* The nav has a fixed width, so a long name or detail wraps instead of being cut. */
  .nav-row.account {
    height: auto;
    min-height: 48px;
    padding-block: 6px;
  }

  .account :global(svg) {
    flex-shrink: 0;
  }

  .account span {
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .account small {
    font-size: 12px;
    color: var(--text-2);
    transition: color 120ms ease-out;
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

  /* Drawn with the theme's colors instead of the platform's. The open list stays
     native: WebView2 and Chromium use the option colors below, while WebKit's menus on
     macOS and Linux follow only color-scheme. */
  .select {
    position: relative;
    display: inline-flex;
    min-width: 0;
    max-width: 60%;
  }

  select {
    appearance: none;
    min-width: 0;
    height: 26px;
    padding: 0 28px 0 10px;
    border: 0;
    border-radius: 6px;
    background: var(--active);
    font: inherit;
    color: var(--text);
    text-overflow: ellipsis;
  }

  select:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  option {
    background: var(--reader-bg);
    color: var(--text);
  }

  .select :global(svg) {
    position: absolute;
    top: 50%;
    right: 8px;
    translate: 0 -50%;
    color: var(--text-2);
    pointer-events: none;
  }

  /* An editable value, drawn like the read-only ones until focused. */
  input.text {
    flex: 1;
    min-width: 0;
    height: 26px;
    padding: 0 8px;
    border: 0;
    border-radius: 6px;
    background: none;
    font: inherit;
    color: var(--text-2);
    text-align: right;
  }

  input.text:hover {
    background: var(--hover);
  }

  input.text:focus {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
    background: var(--reader-bg);
    color: var(--text);
    text-align: left;
  }

  .buttons {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  /* Apart from Cancel and Remove, at the start. */
  .button.export {
    margin-right: auto;
  }

  /* A row that opens a page. */
  .row.link {
    width: 100%;
    text-align: left;
    color: inherit;
  }

  .row.link:hover {
    background: var(--hover);
  }

  .row.link :global(svg) {
    color: var(--text-3);
  }

  .back {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    margin: 0 0 4px -4px;
    padding: 2px 6px 2px 2px;
    border-radius: 6px;
    font-weight: 600;
    color: var(--accent);
  }

  .back:hover {
    background: var(--hover);
  }

  .intro {
    margin: 0 0 10px;
    color: var(--text-2);
  }

  .feed {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .feed-title {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .thumb {
    display: grid;
    place-items: center;
    flex: none;
    width: 16px;
    height: 16px;
    border-radius: 3px;
    overflow: hidden;
    color: var(--text-3);
  }

  .thumb img {
    width: 100%;
    height: 100%;
  }

  /* A whole group's width, as a row of its own. */
  .button.wide {
    width: 100%;
    height: 40px;
    border: 1px solid var(--separator);
    border-radius: 10px;
    background: var(--hover);
  }

  .button.wide:hover {
    background: var(--active);
  }

  /* Leads to a destructive action, so red text; the confirmation's button is filled red. */
  .button.remove {
    color: var(--error);
  }

  .error {
    margin: 0 28px;
    color: var(--error);
  }

  .confirm {
    padding: 12px;
    animation: appear 140ms ease-out;
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
    background: var(--switch-knob);
    box-shadow: 0 0 0 0.5px var(--thumb-edge), 0 1px 2px rgb(0 0 0 / 0.2);
    transition: transform 120ms ease;
  }

  input[role='switch']:checked {
    background: var(--accent);
  }

  input[role='switch']:checked::after {
    transform: translateX(14px);
  }
</style>
