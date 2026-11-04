<script lang="ts">
  import type { Snippet } from 'svelte'
  import { errorMessage } from './api'
  import { apiUrl, sentence } from './format'
  import { macWindow } from './platform'
  import type { CommandError } from './types'

  interface Props {
    /** Signs in. A thrown `CommandError` shows under the form. */
    submit: (server: string, username: string, password: string) => Promise<void>
    /** Fills in the form, to sign in to this account again. */
    account?: { server: string; username: string } | null
    /** Shown until the next attempt. */
    initialError?: string | null
    /** In the Settings window, instead of the whole main window. */
    embedded?: boolean
    /** Under the form, such as other ways to add an account. */
    children?: Snippet
  }

  let { submit: signIn, account = null, initialError = null, embedded = false, children }: Props = $props()

  // Filled in once; the user's typing wins after that.
  // svelte-ignore state_referenced_locally
  let server = $state(account?.server ?? '')
  // svelte-ignore state_referenced_locally
  let username = $state(account?.username ?? '')
  let password = $state('')
  let busy = $state(false)
  // svelte-ignore state_referenced_locally
  let error = $state<string | null>(initialError)

  const messages: Record<CommandError['kind'], string> = {
    unauthorized: 'Wrong username or API password.',
    badUrl: 'Enter the server address, such as https://rss.example.com.',
    network: 'Could not reach the server. Check the address and your connection.',
    noFeed: '',
    other: '',
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault()
    busy = true
    error = null
    try {
      server = apiUrl(server)
      await signIn(server, username, password)
      password = ''
    } catch (err) {
      // Tauri's own errors are plain strings.
      error = messages[(err as CommandError | null)?.kind ?? 'other'] || sentence(errorMessage(err))
    } finally {
      busy = false
    }
  }
</script>

{#if macWindow && !embedded}
  <!-- The title bar is hidden on macOS: an empty strip to drag the window by. Not the
       whole screen, so text stays selectable and the scrollbar keeps working. -->
  <div class="drag-strip" data-tauri-drag-region></div>
{/if}
<svelte:element this={embedded ? 'div' : 'main'} class="sign-in" class:embedded>
  <form onsubmit={submit}>
    {#if !embedded}<h1>Sign in to your sync service</h1>{/if}
    <p class="intro">
      Skvaller reads your feeds from a service that speaks the Google Reader API, such as FreshRSS.
    </p>

    <label>
      <span>Server</span>
      <input
        bind:value={server}
        placeholder="https://rss.example.com"
        autocapitalize="off"
        autocorrect="off"
        spellcheck="false"
        required
      />
    </label>
    <label>
      <span>Username</span>
      <input bind:value={username} autocapitalize="off" autocorrect="off" spellcheck="false" required />
    </label>
    <label>
      <span>API password</span>
      <input type="password" bind:value={password} required />
    </label>
    <p class="hint">
      In FreshRSS, allow API access under Settings → Authentication, then set an API password under
      Profile. Skvaller keeps only a sign-in token in your keychain, never the password.
    </p>

    {#if error}<p class="error" role="alert">{error}</p>{/if}

    <button class="primary" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</button>
    {@render children?.()}
  </form>
</svelte:element>

<style>
  .drag-strip {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    height: var(--toolbar-height);
  }

  .sign-in {
    display: grid;
    place-items: center;
    height: 100%;
    padding: 24px;
    background: var(--sidebar-bg);
    overflow-y: auto;
  }

  .sign-in.embedded {
    display: block;
    height: auto;
    padding: 0;
    background: none;
    overflow: visible;
  }

  .embedded form {
    max-width: none;
  }

  form {
    display: flex;
    flex-direction: column;
    gap: 12px;
    width: 100%;
    max-width: 360px;
  }

  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 800;
    letter-spacing: -0.01em;
  }

  .intro,
  .hint {
    margin: 0;
    color: var(--text-2);
    line-height: 1.45;
  }

  .intro {
    margin-bottom: 8px;
  }

  .hint {
    font-size: 12px;
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
