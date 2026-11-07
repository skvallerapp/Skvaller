<script lang="ts">
  import ChevronLeft from '@lucide/svelte/icons/chevron-left'
  import { errorMessage } from './api'
  import { sentence, serverUrl } from './format'
  import { t } from './i18n.svelte'
  import ServiceLogo from './ServiceLogo.svelte'
  import type { Service } from './services'
  import type { CommandError } from './types'

  interface Props {
    service: Service
    /** Signs in. A thrown `CommandError` shows under the form. */
    submit: (server: string, username: string, password: string) => Promise<void>
    /** Fills in the form, to sign in to this account again. Its description is the
     * heading: a saved address can't tell Miniflux from any other server. */
    account?: { server: string; username: string; description: string } | null
    /** Shown until the next attempt. */
    initialError?: string | null
    /** Back to the list of services; none when signing in again. */
    back?: () => void
    /** In the Settings window: a smaller heading. */
    embedded?: boolean
  }

  let { service, submit: signIn, account = null, initialError = null, back, embedded = false }: Props = $props()

  // Filled in once; the user's typing wins after that.
  // svelte-ignore state_referenced_locally
  let server = $state(account?.server ?? '')
  // svelte-ignore state_referenced_locally
  let username = $state(account?.username ?? '')
  let password = $state('')
  let busy = $state(false)
  // svelte-ignore state_referenced_locally
  let error = $state<string | null>(initialError)

  const messages = $derived<Record<CommandError['kind'], string>>({
    unauthorized: t(`signIn.wrong.${service.username}.${service.password}`),
    badUrl: t('signIn.badUrl'),
    network: service.server ? t('signIn.networkService', { service: service.name }) : t('signIn.networkServer'),
    noFeed: '',
    other: '',
  })

  async function submit(e: SubmitEvent) {
    e.preventDefault()
    busy = true
    error = null
    try {
      if (!service.server) server = serverUrl(server)
      await signIn(service.server ?? server, username, password)
      password = ''
    } catch (err) {
      // Tauri's own errors are plain strings.
      error = messages[(err as CommandError | null)?.kind ?? 'other'] || sentence(errorMessage(err))
    } finally {
      busy = false
    }
  }
</script>

<!-- Escape from any field goes back to the list, like Back. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<form
  onsubmit={submit}
  onkeydown={(e) => {
    // Not while signing in: its result would have nowhere to show.
    if (e.key === 'Escape' && back && !busy && !e.isComposing) {
      e.preventDefault()
      e.stopPropagation()
      back()
    }
  }}
>
  <header>
    {#if back}
      <button type="button" class="back" onclick={back} disabled={busy} aria-label={t('signIn.allServices')}>
        <ChevronLeft size={18} />
      </button>
    {/if}
    <ServiceLogo {service} size={embedded ? 24 : 28} />
    <svelte:element this={embedded ? 'h2' : 'h1'} class="title" class:small={embedded}>
      {account?.description || service.name}
    </svelte:element>
  </header>

  {#if !service.server}
    <label>
      <span>{t('signIn.server')}</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        autofocus={!account}
        bind:value={server}
        placeholder="https://rss.example.com"
        autocapitalize="off"
        autocorrect="off"
        spellcheck="false"
        required
      />
    </label>
  {/if}
  <label>
    <span>{t(`signIn.${service.username}`)}</span>
    <!-- svelte-ignore a11y_autofocus -->
    <input
      bind:value={username}
      type={service.username === 'email' ? 'email' : 'text'}
      autofocus={!!service.server && !account}
      autocapitalize="off"
      autocorrect="off"
      spellcheck="false"
      required
    />
  </label>
  <label>
    <span>{t(`signIn.${service.password}`)}</span>
    <!-- Signing in again: only the password is missing. -->
    <!-- svelte-ignore a11y_autofocus -->
    <input type="password" autofocus={!!account} bind:value={password} required />
  </label>
  <p class="hint">
    {t('common.twoSentences', { first: t(`services.${service.id}.help`), second: t('signIn.tokenOnly') })}
  </p>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  <button class="primary" disabled={busy}>{busy ? t('signIn.signingIn') : t('signIn.submit')}</button>
</form>

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  header {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
  }

  .title {
    margin: 0;
    padding: 0;
    font-size: 22px;
    font-weight: 800;
    letter-spacing: -0.01em;
    text-transform: none;
    color: var(--text);
  }

  .title.small {
    font-size: 17px;
  }

  .back {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    margin-left: -6px;
    border-radius: 7px;
    color: var(--text-2);
  }

  .back:disabled {
    opacity: 0.4;
  }

  .back:hover:not(:disabled) {
    background: var(--hover);
    color: var(--text);
  }

  .hint {
    margin: 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--text-2);
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-weight: 600;
    color: var(--text-2);
  }

  input {
    height: 32px;
    padding: 0 10px;
    border: 1px solid var(--separator);
    border-radius: 7px;
    background: var(--reader-bg);
    color: var(--text);
    font: inherit;
    font-weight: 400;
  }

  input:focus {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
  }

  .error {
    margin: 0;
    color: var(--error);
  }

  .primary {
    height: 34px;
    margin-top: 4px;
    border-radius: 7px;
    background: var(--accent);
    color: var(--accent-text);
    font-weight: 600;
  }

  .primary:disabled {
    opacity: 0.6;
  }
</style>
