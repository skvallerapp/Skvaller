<script lang="ts">
  import { apiUrl } from './format'
  import { macWindow } from './platform'
  import { store } from './store.svelte'
  import type { CommandError } from './types'

  let server = $state(store.account?.server ?? '')
  let username = $state(store.account?.username ?? '')
  let password = $state('')
  let busy = $state(false)
  let error = $state<string | null>(
    store.status.unauthorized
      ? 'The server no longer accepts the sign-in. Sign in again.'
      : store.status.error,
  )

  const messages: Record<CommandError['kind'], string> = {
    unauthorized: 'Wrong username or API password.',
    badUrl: 'Enter the server address, such as https://rss.example.com.',
    network: 'Could not reach the server. Check the address and your connection.',
    other: '',
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault()
    busy = true
    error = null
    try {
      server = apiUrl(server)
      await store.signIn(server, username, password)
    } catch (err) {
      const { kind, message } = err as CommandError
      error = messages[kind] || message
    } finally {
      busy = false
    }
  }
</script>

{#if macWindow}
  <!-- The title bar is hidden on macOS: an empty strip to drag the window by. Not the
       whole screen, so text stays selectable and the scrollbar keeps working. -->
  <div class="drag-strip" data-tauri-drag-region></div>
{/if}
<main class="sign-in">
  <form onsubmit={submit}>
    <h1>Sign in to your sync service</h1>
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
  </form>
</main>

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
