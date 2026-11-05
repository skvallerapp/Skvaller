<script lang="ts">
  import Rss from '@lucide/svelte/icons/rss'
  import type { Service } from './services'

  let { service, size = 32 }: { service: Service; size?: number } = $props()
  // Quoted: Vite inlines small SVGs as data URLs with spaces in them.
  const mask = $derived(`url("${service.logo}")`)
</script>

{#if service.logo}
  <!-- A mask, so the logo takes the text color like the icons around it. -->
  <span
    class="logo"
    aria-hidden="true"
    style:width="{size}px"
    style:height="{size}px"
    style:-webkit-mask-image={mask}
    style:mask-image={mask}
  ></span>
{:else}
  <Rss size={size * 0.85} aria-hidden="true" />
{/if}

<style>
  .logo {
    display: block;
    flex-shrink: 0;
    background: currentColor;
    -webkit-mask-size: contain;
    mask-size: contain;
    -webkit-mask-repeat: no-repeat;
    mask-repeat: no-repeat;
    -webkit-mask-position: center;
    mask-position: center;
  }
</style>
