<!-- The unread mark: a small circle, filled while unread and outlined once read. It
     takes a Lucide icon's size and draws inside the same box, so it fits icon slots. -->
<script lang="ts">
  let {
    size = 24,
    read = false,
    class: className,
  }: { size?: number; read?: boolean; strokeWidth?: number; class?: string } = $props()
</script>

<svg
  class={className}
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill={read ? 'none' : 'currentColor'}
  stroke="currentColor"
  stroke-width="1.6"
  aria-hidden="true"
>
  <circle cx="12" cy="12" r="7.5" />
</svg>
