<script lang="ts">
  // Under the sign-in form: the local account, for someone without a sync service.
  import { errorMessage } from './api'
  import { sentence } from './format'

  let { add, label = 'Use Without an Account' }: { add: () => Promise<void>; label?: string } = $props()
  let busy = $state(false)
  let error = $state<string | null>(null)

  async function onclick() {
    busy = true
    error = null
    try {
      await add()
    } catch (e) {
      error = sentence(errorMessage(e))
    } finally {
      busy = false
    }
  }
</script>

<div class="local">
  <p>No sync service? Skvaller can fetch your feeds itself and keep them on this device only.</p>
  <button type="button" class="secondary" disabled={busy} {onclick}>{label}</button>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</div>

<style>
  .local {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 12px;
    padding-top: 16px;
    border-top: 1px solid var(--separator);
  }

  p {
    margin: 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--text-2);
  }

  .secondary {
    height: 34px;
    border-radius: 7px;
    background: var(--active);
    font-weight: 600;
  }

  .secondary:disabled {
    opacity: 0.6;
  }

  .error {
    color: var(--error);
  }
</style>
