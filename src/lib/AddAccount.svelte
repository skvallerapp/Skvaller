<script lang="ts">
  // The services to add an account for, then the chosen one's sign-in form. On the
  // first-run screen, for an account to sign in to again, and in Settings.
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import Laptop from '@lucide/svelte/icons/laptop'
  import { tick } from 'svelte'
  import { errorMessage } from './api'
  import { sentence } from './format'
  import { t } from './i18n.svelte'
  import { macWindow } from './platform'
  import ServiceLogo from './ServiceLogo.svelte'
  import { hosted, selfHosted, serviceFor, type Service } from './services'
  import SignIn from './SignIn.svelte'

  interface Props {
    submit: (server: string, username: string, password: string) => Promise<void>
    /** Adds the local account; none if there is one already. */
    addLocal?: (() => Promise<void>) | null
    /** An account to sign in to again: its form, without the list. */
    account?: { server: string; username: string; description: string } | null
    /** Why the account is signed out, or why the last attempt failed: over the list, or
     * in the form of the account to sign in to again. */
    initialError?: string | null
    /** In the Settings window, instead of the whole main window. */
    embedded?: boolean
  }

  let { submit, addLocal = null, account = null, initialError = null, embedded = false }: Props = $props()

  let chosen = $state<Service | null>(null)
  let list = $state<HTMLElement>()
  let addingLocal = $state(false)
  let localError = $state<string | null>(null)

  async function back() {
    const id = chosen?.id
    chosen = null
    await tick()
    list?.querySelector<HTMLElement>(`[data-service="${id}"]`)?.focus()
  }

  async function local() {
    if (!addLocal) return
    addingLocal = true
    localError = null
    try {
      await addLocal()
    } catch (e) {
      localError = sentence(errorMessage(e))
    } finally {
      addingLocal = false
    }
  }
</script>

{#snippet row(service: Service)}
  <button
    type="button"
    class="service"
    data-service={service.id}
    disabled={addingLocal}
    onclick={() => (chosen = service)}
  >
    <ServiceLogo {service} />
    <span class="text"><strong>{service.name}</strong><small>{service.detail}</small></span>
    <ChevronRight size={16} class="chevron" />
  </button>
{/snippet}

{#if macWindow && !embedded}
  <!-- The title bar is hidden on macOS: an empty strip to drag the window by. Not the
       whole screen, so text stays selectable and the scrollbar keeps working. -->
  <div class="drag-strip" data-tauri-drag-region></div>
{/if}
<svelte:element this={embedded ? 'div' : 'main'} class="add-account" class:embedded>
  <div class="column">
    {#if account}
      <SignIn service={serviceFor(account.server)} {submit} {account} {initialError} {embedded} />
    {:else if chosen}
      <SignIn service={chosen} {submit} {embedded} {back} />
    {:else}
      {#if !embedded}
        <h1>{t('addAccount.title')}</h1>
        <p class="intro">{t('addAccount.intro')}</p>
      {/if}
      {#if initialError}<p class="error" role="alert">{initialError}</p>{/if}
      <div class="groups" bind:this={list}>
        <section>
          <h2 class="heading">{t('addAccount.services')}</h2>
          {#each hosted as service (service.id)}{@render row(service)}{/each}
        </section>
        <section>
          <h2 class="heading">{t('addAccount.selfHosted')}</h2>
          {#each selfHosted as service (service.id)}{@render row(service)}{/each}
        </section>
        {#if addLocal}
          <section>
            <h2 class="heading">{t('addAccount.withoutAccount')}</h2>
            <button type="button" class="service" disabled={addingLocal} onclick={local}>
              <Laptop size={27} aria-hidden="true" />
              <span class="text">
                <strong>{addingLocal ? t('common.adding') : t('addAccount.local')}</strong>
                <small>{t('addAccount.localDetail')}</small>
              </span>
              <ChevronRight size={16} class="chevron" />
            </button>
            {#if localError}<p class="error" role="alert">{localError}</p>{/if}
          </section>
        {/if}
      </div>
    {/if}
  </div>
</svelte:element>

<style>
  .drag-strip {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    height: var(--toolbar-height);
  }

  .add-account {
    display: grid;
    place-items: center;
    height: 100%;
    padding: 24px;
    background: var(--sidebar-bg);
    overflow-y: auto;
  }

  .add-account.embedded {
    display: block;
    height: auto;
    padding: 0;
    background: none;
    overflow: visible;
  }

  .column {
    display: flex;
    flex-direction: column;
    gap: 12px;
    width: 100%;
    max-width: 360px;
  }

  .embedded .column {
    max-width: none;
  }

  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 800;
    letter-spacing: -0.01em;
  }

  .intro {
    margin: 0 0 8px;
    color: var(--text-2);
    line-height: 1.45;
  }

  .groups {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  section {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  /* As the Settings window's group headings, lined up with the names. */
  .heading {
    margin: 0 0 4px;
    padding-left: 54px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.03em;
    text-transform: uppercase;
    color: var(--text-3);
  }

  .service {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 8px 10px;
    border-radius: 9px;
    color: var(--text);
    text-align: left;
  }

  .service:hover:not(:disabled) {
    background: var(--hover);
  }

  .service:active:not(:disabled) {
    background: var(--active);
  }

  .service:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  .service:disabled {
    opacity: 0.6;
  }

  .text {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  strong {
    font-weight: 600;
  }

  small {
    font-size: 12px;
    color: var(--text-2);
  }

  .service :global(.chevron) {
    flex-shrink: 0;
    color: var(--text-3);
  }

  .service > :global(svg:first-child) {
    flex-shrink: 0;
    width: 32px;
  }

  .error {
    margin: 0;
    color: var(--error);
  }
</style>
