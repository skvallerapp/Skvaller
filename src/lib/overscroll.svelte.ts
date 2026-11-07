// Pulling a list past its top or bottom, by trackpad or touch, stretches it away from the
// edge, and it springs back when let go. A pull at the top that goes far enough refreshes,
// if it began with the list at the top. A trackpad's momentum that reaches an edge bounces
// the list off it, as macOS does. A mouse wheel doesn't stretch it.
// The browser's own elastic overscroll is off (`overscroll-behavior: none`), so all three
// platforms work alike. With animations off the list stays put, and a long pull at the top
// refreshes at once.
import { untrack } from 'svelte'
import { animate } from './motion'

/** The rubber band's give: the further the pull, the less the list follows. */
const GIVE = 0.55
/** How far down a pull at the top must stretch the list to refresh. */
const TRIGGER = 90
/** Where the list waits while it refreshes. */
const HOLD = 48
/** A trackpad sends no event once the fingers and their momentum stop: after this long
 * without one, the pull is let go. */
const WHEEL_IDLE = 120
/** A trackpad's scrolling is accelerated, so a quick swipe goes far: it pulls this much
 * less than a finger on a touchscreen does. */
const WHEEL_PULL = 0.4
/** A trackpad accelerates a quick flick to huge events. One pulls this much at most, so
 * the pull follows how far the fingers go, not how fast. */
const WHEEL_MAX = 40
/** After the fingers lift, a trackpad's momentum goes on as events that shrink smoothly,
 * give or take a pixel or a doubled event. This many smaller ones are momentum, not a hand. */
const COASTING = 3
/** Momentum shrinks by less than this from one event to the next. A hand slowing down
 * drops more at once. */
const SMOOTH = 0.8
/** Fingers back on the trackpad grow the events this many times in a row; momentum never
 * does. */
const HAND_BACK = 2
/** A mouse wheel turns in notches of this much `wheelDeltaY`, which Chromium and WebKit
 * still report; its `deltaY` doesn't tell it from a trackpad. */
const NOTCH = 120
/** Momentum that reaches an edge bounces the list off it this far at most. */
const BOUNCE_MAX = 60
/** How long the list takes to spring back, and to bounce out. */
const SETTLE_MS = 400
const BOUNCE_MS = 120

/** How far the list stretches for a pull of `x`, at most `d`. Signed, as `x` is. */
function stretch(x: number, d: number) {
  return Math.sign(x) * (1 - 1 / ((Math.abs(x) * GIVE) / d + 1)) * d
}

/** The pull that stretches the list `y`: the inverse of `stretch`. */
function pullFor(y: number, d: number) {
  return Math.sign(y) * (d / GIVE) * (1 / (1 - Math.min(Math.abs(y) / d, 0.99)) - 1)
}

export class Overscroll {
  /** How far the list is stretched from its edge, in px: down from the top is positive. */
  offset = $state(0)
  /** A refresh runs, with the list held at HOLD, or the list springs back after one. */
  refreshing = $state(false)
  /** This gesture began with the list at the top and stayed there, so pulling down can
   * refresh. */
  fromTop = $state(false)
  /** How far a pull at the top is towards a refresh, 0 to 1. */
  readonly progress = $derived(Math.min(1, Math.max(0, this.offset) / TRIGGER))
  /** Letting go refreshes, or a refresh runs. */
  readonly armed = $derived(this.progress === 1 || this.refreshing)
  readonly hold = HOLD

  #refresh: () => Promise<void>
  #el: HTMLElement | null = null
  /** The pull so far, before the rubber band. */
  #pull = 0
  /** With animations off, this gesture already refreshed. */
  #spent = false
  #frame = 0
  #idle = 0
  /** The wheel and touch listeners can keep the list from scrolling. */
  #blocking: boolean | null = null
  /** When the last wheel event came. */
  #wheelAt = -Infinity
  /** This wheel gesture is a mouse's. */
  #mouse = false
  /** The size of the last wheel event. */
  #wheelSize = 0
  /** How many wheel events in a row shrank smoothly. An equal one doesn't end the run. */
  #shrinking = 0
  /** How many wheel events in a row were larger than the one before. */
  #growing = 0
  /** The wheel events are momentum: the fingers have lifted. */
  #coasting = false
  /** This momentum already bounced the list off an edge, or sprang it back. */
  #bounced = false
  /** The pull of that run: momentum, once it shows as such. */
  #coasted = 0
  #touchY: number | null = null
  /** This touch was kept from scrolling the list, so it scrolls it here instead. */
  #touchOwned = false

  constructor(refresh: () => Promise<void>) {
    this.#refresh = refresh
  }

  /** For `{@attach}` on the scrolling element. */
  attach = (el: HTMLElement) => {
    this.#el = el
    // Svelte runs an attachment in an effect: reading the offset here would run it again,
    // and its cleanup would stop the spring back.
    untrack(this.#updateListeners)
    const passive = { passive: true }
    el.addEventListener('scroll', this.#updateListeners, passive)
    el.addEventListener('touchstart', this.#onTouchStart, passive)
    el.addEventListener('touchend', this.#onTouchEnd, passive)
    el.addEventListener('touchcancel', this.#onTouchCancel, passive)
    return () => {
      el.removeEventListener('wheel', this.#onWheel)
      el.removeEventListener('touchmove', this.#onTouchMove)
      el.removeEventListener('scroll', this.#updateListeners)
      el.removeEventListener('touchstart', this.#onTouchStart)
      el.removeEventListener('touchend', this.#onTouchEnd)
      el.removeEventListener('touchcancel', this.#onTouchCancel)
      cancelAnimationFrame(this.#frame)
      clearTimeout(this.#idle)
      this.#el = null
      this.#blocking = null
    }
  }

  /** Makes the wheel and touch listeners able to keep the list from scrolling only while
   * it's at an edge or stretched, or a touch scrolls it here, so they don't hold up its
   * scrolling elsewhere. */
  #updateListeners = () => {
    const el = this.#el
    if (!el) return
    const blocking = this.offset !== 0 || this.#touchOwned || this.#atEdge(1) || this.#atEdge(-1)
    if (blocking === this.#blocking) return
    this.#blocking = blocking
    const options = { passive: !blocking }
    el.removeEventListener('wheel', this.#onWheel)
    el.removeEventListener('touchmove', this.#onTouchMove)
    el.addEventListener('wheel', this.#onWheel, options)
    el.addEventListener('touchmove', this.#onTouchMove, options)
  }

  /** Takes `delta` px of pull (down positive), if the list is at that edge or already
   * stretched. Returns whether it took it. */
  #take(delta: number): boolean {
    const el = this.#el
    if (!el || this.refreshing || delta === 0) return false
    const d = el.clientHeight
    if (this.#frame) {
      cancelAnimationFrame(this.#frame)
      this.#frame = 0
      // Caught while springing back at its edge, the pull goes on from where the list is.
      // Away from it, as after a refresh the list was scrolled from, it's done.
      if (this.#atEdge(this.offset)) this.#pull = pullFor(this.offset, d)
      else this.offset = 0
    }
    if (this.#pull === 0 && !this.#atEdge(delta)) return false
    const next = this.#pull + delta
    // Pushed back past the edge, the list rests there.
    this.#pull = this.#pull !== 0 && Math.sign(next) !== Math.sign(this.#pull) ? 0 : next
    if (animate()) this.offset = stretch(this.#pull, d)
    // Momentum before it shows as such doesn't count, as when the list stretches.
    else if (stretch(this.#pull - this.#coasted, d) >= TRIGGER && this.fromTop && !this.#spent) {
      this.#spent = true
      this.#start()
    }
    return true
  }

  /** Whether the list is at the edge that a pull of `delta` (down positive) pulls from. */
  #atEdge(delta: number) {
    const el = this.#el
    if (!el) return false
    return delta > 0 ? el.scrollTop <= 0 : el.scrollTop >= el.scrollHeight - el.clientHeight - 1
  }

  #release = () => {
    clearTimeout(this.#idle)
    const pulled = this.#el ? stretch(this.#pull, this.#el.clientHeight) : 0
    this.#pull = 0
    if (this.refreshing) return
    // With animations off, a long enough pull already refreshed.
    if (pulled >= TRIGGER && this.fromTop && animate()) this.#start()
    else this.#settle(0)
  }

  #start() {
    this.refreshing = true
    this.#settle(HOLD)
    this.#refresh().finally(() => this.#settle(0, SETTLE_MS, () => (this.refreshing = false)))
  }

  /** Springs the list to `target`, then calls `then`. With animations off, the list
   * stays put. */
  #settle(target: number, ms = SETTLE_MS, then?: () => void) {
    cancelAnimationFrame(this.#frame)
    this.#frame = 0
    const done = () => {
      then?.()
      this.#updateListeners()
    }
    const from = this.offset
    if (!animate() || from === target) {
      this.offset = animate() ? target : 0
      return done()
    }
    const start = performance.now()
    const step = (now: number) => {
      const t = Math.min(1, (now - start) / ms)
      this.offset = from + (target - from) * (1 - (1 - t) ** 3)
      this.#frame = t < 1 ? requestAnimationFrame(step) : 0
      if (t === 1) done()
    }
    this.#frame = requestAnimationFrame(step)
  }

  #onWheel = (e: WheelEvent) => {
    // A pinch zooms.
    if (e.ctrlKey || e.deltaMode !== WheelEvent.DOM_DELTA_PIXEL) return
    // A pause in the events starts a new gesture. A mouse's starts with a whole notch, a
    // trackpad's small; later trackpad events can look like notches too.
    const began = e.timeStamp - this.#wheelAt > WHEEL_IDLE
    if (began) {
      this.#began()
      const notches = (e as WheelEvent & { wheelDeltaY?: number }).wheelDeltaY
      this.#mouse = !!notches && notches % NOTCH === 0
    }
    this.#wheelAt = e.timeStamp
    if (this.#mouse) return
    if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) {
      // Chromium lets a gesture keep the list from scrolling only if its first event did.
      // The list doesn't scroll sideways, so keeping it from that costs nothing.
      if (began && e.cancelable && this.#blocking) e.preventDefault()
      return
    }
    this.#leftTop()
    const size = Math.abs(e.deltaY)
    const shrank = size < this.#wheelSize && size >= this.#wheelSize * SMOOTH
    const same = size === this.#wheelSize
    /** This event may be momentum's: it goes on a run of smoothly shrinking ones. */
    const momentum = shrank || (same && this.#shrinking > 0)
    if (shrank) this.#shrinking++
    else if (!same) {
      this.#shrinking = 0
      this.#coasted = 0
    }
    this.#growing = size > this.#wheelSize ? this.#growing + 1 : 0
    this.#wheelSize = size
    if (this.#shrinking >= COASTING && !this.#coasting) {
      this.#coasting = true
      // The momentum before it showed as such doesn't count towards a refresh.
      this.#pull -= this.#coasted
    }
    // The fingers are back, as a new gesture.
    if (this.#coasting && this.#growing >= HAND_BACK) this.#began()
    if (this.#coasting) {
      // While it refreshes, the list stays held: momentum may scroll it, but neither
      // bounces nor releases it.
      if (this.refreshing) return
      if (this.#pull !== 0 || (this.offset !== 0 && this.#atEdge(this.offset))) {
        // The fingers lifted: a stretched list springs back at once, as it does natively,
        // and the momentum neither stretches it nor scrolls it.
        e.preventDefault()
        this.#bounced = true
        if (this.#frame === 0) this.#release()
      } else if (!this.#bounced && this.#atEdge(-e.deltaY) && animate()) {
        // Momentum reached an edge: it bounces the list off it.
        this.#bounced = true
        e.preventDefault()
        this.#settle(-Math.sign(e.deltaY) * Math.min(size, BOUNCE_MAX), BOUNCE_MS, () => this.#settle(0))
      }
      return
    }
    // Where the listener can't keep the list from scrolling, as in a gesture that began
    // away from an edge, the browser scrolls it: stretching it too would show a gap.
    if (!e.cancelable || !this.#blocking) return this.#updateListeners()
    const pull = -Math.sign(e.deltaY) * Math.min(size, WHEEL_MAX) * WHEEL_PULL
    if (!this.#take(pull)) return
    if (momentum) this.#coasted += pull
    e.preventDefault()
    clearTimeout(this.#idle)
    this.#idle = window.setTimeout(this.#release, WHEEL_IDLE)
  }

  /** A gesture begins: a touch, or the wheel after a pause or with the fingers back. */
  #began() {
    this.fromTop = this.#atEdge(1)
    this.#spent = false
    this.#wheelSize = 0
    this.#shrinking = 0
    this.#growing = 0
    this.#coasting = false
    this.#bounced = false
    this.#coasted = 0
  }

  /** A gesture that scrolled the list away from the top can't refresh. */
  #leftTop() {
    if (this.fromTop && !this.#atEdge(1)) this.fromTop = false
  }

  #onTouchStart = (e: TouchEvent) => {
    this.#touchY = e.touches.length === 1 ? e.touches[0].clientY : null
    this.#touchOwned = false
    this.#began()
    this.#updateListeners()
  }

  #onTouchMove = (e: TouchEvent) => {
    if (this.#touchY === null || e.touches.length !== 1) return
    const y = e.touches[0].clientY
    const delta = y - this.#touchY
    this.#touchY = y
    this.#leftTop()
    // As with the wheel: only where the listener keeps the list from scrolling.
    const blocks = e.cancelable && this.#blocking
    if (blocks && this.#take(delta)) {
      e.preventDefault()
      this.#touchOwned = true
    } else if (this.#touchOwned && this.#el) {
      // The browser doesn't scroll a touch that began as a pull.
      this.#el.scrollTop -= delta
      if (e.cancelable) e.preventDefault()
    } else if (!blocks) this.#updateListeners()
  }

  #onTouchEnd = (e: TouchEvent) => {
    if (e.touches.length > 0) return
    this.#touchY = null
    this.#touchOwned = false
    this.#release()
  }

  /** The system took the touch over, as for a gesture of its own: no refresh. */
  #onTouchCancel = (e: TouchEvent) => {
    this.fromTop = false
    this.#onTouchEnd(e)
  }
}
