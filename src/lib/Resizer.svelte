<script lang="ts">
  interface Props {
    label: string
    value: number
    min: number
    max: number
    onchange: (value: number) => void
    onend: () => void
    ondragging?: (dragging: boolean) => void
  }

  let { label, value, min, max, onchange, onend, ondragging }: Props = $props()

  let startX = 0
  let startValue = 0
  let dragging = $state(false)

  const clamp = (n: number) => Math.min(max, Math.max(min, n))

  function down(e: PointerEvent) {
    if (e.button !== 0) return
    e.preventDefault()
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
    startX = e.clientX
    startValue = value
    dragging = true
    ondragging?.(true)
  }

  function move(e: PointerEvent) {
    if (dragging) onchange(clamp(startValue + e.clientX - startX))
  }

  function up() {
    if (!dragging) return
    dragging = false
    ondragging?.(false)
    onend()
  }

  function key(e: KeyboardEvent) {
    const step = e.shiftKey ? 40 : 10
    if (e.key === 'ArrowLeft') onchange(clamp(value - step))
    else if (e.key === 'ArrowRight') onchange(clamp(value + step))
    else return
    e.preventDefault()
    onend()
  }
</script>

<!-- A focusable separator is an interactive widget in ARIA; Svelte's lint doesn't know that. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="resizer"
  class:dragging
  role="separator"
  tabindex="0"
  aria-label={label}
  aria-orientation="vertical"
  aria-valuenow={value}
  aria-valuemin={min}
  aria-valuemax={max}
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  onkeydown={key}
></div>

<style>
  .resizer {
    position: relative;
    flex: none;
    width: 1px;
    background: var(--separator);
    z-index: 2;
  }

  /* A wider invisible hit area than the 1px line. */
  .resizer::before {
    content: '';
    position: absolute;
    inset: 0 -4px;
    cursor: col-resize;
  }

  .resizer.dragging,
  .resizer:focus-visible {
    background: var(--accent);
    outline: none;
  }
</style>
