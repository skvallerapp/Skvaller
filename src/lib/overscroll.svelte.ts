// Pulling a list past its top or bottom, by trackpad or touch, stretches it away from the
// edge, and it springs back when let go. A pull at the top that goes far enough refreshes,
// if it began with the list at the top. A trackpad's momentum that reaches an edge bounces
// the list off it, as macOS does.
// The browser's own elastic overscroll is off (`overscroll-behavior: none`), so all three
// platforms work alike. With animations off the list stays put, and a long pull at the top
// refreshes at once.
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
const SETTLE_MS = 400
/** After the fingers lift, a trackpad's momentum goes on as events that shrink smoothly,
 * give or take a pixel or a doubled event. This many smaller ones are momentum, not a hand. */
const COASTING = 3
/** Momentum shrinks by less than this from one event to the next. A hand slowing down
 * drops more at once. */
const SMOOTH = 0.8
/** Fingers back on the trackpad grow the events this many times in a row; momentum never
 * does. */
const HAND_BACK = 2
/** Momentum that reaches an edge bounces the list off it this far at most, and this fast. */
const BOUNCE_MAX = 60
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
  /** A refresh runs, with the list held at HOLD. */
  refreshing = $state(false)
  /** This gesture began with the list at the top, so pulling down can refresh. */
  fromTop = $state(false)
  /** How far the list stretches to refresh. */
  readonly trigger = TRIGGER
  readonly hold = HOLD

  #refresh: () => Promise<void>
  #el: HTMLElement | null = null
  /** The pull so far, before the rubber band. */
  #pull = 0
  /** With animations off, this pull already refreshed. */
  #spent = false
  #frame = 0
  #idle = 0
  /** When the last wheel event came. */
  #wheelAt = -Infinity
  /** The size of the last wheel event, and how many were smaller than the one before
   * since one was larger. */
  #wheelSize = 0
  #shrinking = 0
  /** How many in a row were larger than the one before. */
  #growing = 0
  /** The wheel events are momentum: the fingers have lifted. */
  #coasting = false
  /** This momentum already bounced the list off an edge, or sprang it back. */
  #bounced = false
  /** The pull since the last larger event: momentum, once it shows as such. */
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
    const active = { passive: false }
    el.addEventListener('wheel', this.#onWheel, active)
    el.addEventListener('touchstart', this.#onTouchStart, { passive: true })
    el.addEventListener('touchmove', this.#onTouchMove, active)
    el.addEventListener('touchend', this.#onTouchEnd)
    el.addEventListener('touchcancel', this.#onTouchEnd)
    return () => {
      el.removeEventListener('wheel', this.#onWheel)
      el.removeEventListener('touchstart', this.#onTouchStart)
      el.removeEventListener('touchmove', this.#onTouchMove)
      el.removeEventListener('touchend', this.#onTouchEnd)
      el.removeEventListener('touchcancel', this.#onTouchEnd)
      cancelAnimationFrame(this.#frame)
      clearTimeout(this.#idle)
      this.#el = null
    }
  }

  /** Takes `delta` px of pull (down positive), if the list is at that edge or already
   * stretched. Returns whether it took it. */
  #take(delta: number): boolean {
    const el = this.#el
    if (!el || this.refreshing || delta === 0) return false
    const d = el.clientHeight
    // Caught while springing back: the pull goes on from where the list is.
    if (this.#frame) {
      cancelAnimationFrame(this.#frame)
      this.#frame = 0
      this.#pull = pullFor(this.offset, d)
    }
    if (this.#pull === 0 && !this.#atEdge(delta)) return false
    const next = this.#pull + delta
    // Pushed back past the edge, the list rests there.
    this.#pull = this.#pull !== 0 && Math.sign(next) !== Math.sign(this.#pull) ? 0 : next
    const offset = stretch(this.#pull, d)
    if (animate()) this.offset = offset
    else if (offset >= TRIGGER && this.fromTop && !this.#spent) {
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
    this.#spent = false
    if (this.refreshing) return
    // With animations off, a long enough pull already refreshed.
    if (pulled >= TRIGGER && this.fromTop && animate()) this.#start()
    else this.#settle(0)
  }

  #start() {
    this.refreshing = true
    this.#settle(HOLD)
    this.#refresh().finally(() => {
      this.refreshing = false
      this.#settle(0)
    })
  }

  /** Springs the list to `target`, then calls `then`. */
  #settle(target: number, ms = SETTLE_MS, then?: () => void) {
    cancelAnimationFrame(this.#frame)
    this.#frame = 0
    const from = this.offset
    if (!animate() || from === target) {
      this.offset = animate() ? target : 0
      return
    }
    const start = performance.now()
    const step = (now: number) => {
      const t = Math.min(1, (now - start) / ms)
      this.offset = from + (target - from) * (1 - (1 - t) ** 3)
      this.#frame = t < 1 ? requestAnimationFrame(step) : 0
      if (t === 1) then?.()
    }
    this.#frame = requestAnimationFrame(step)
  }

  #onWheel = (e: WheelEvent) => {
    // A pinch zooms; a mouse wheel that scrolls by lines doesn't stretch.
    if (e.ctrlKey || e.deltaMode !== WheelEvent.DOM_DELTA_PIXEL) return
    if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) return
    // A pause in the events starts a new gesture.
    if (e.timeStamp - this.#wheelAt > WHEEL_IDLE) this.#wheelBegan()
    this.#wheelAt = e.timeStamp
    const size = Math.abs(e.deltaY)
    if (size > this.#wheelSize) {
      this.#shrinking = 0
      this.#growing++
      this.#coasted = 0
    } else {
      this.#growing = 0
      if (size >= this.#wheelSize * SMOOTH) {
        if (size < this.#wheelSize) this.#shrinking++
      } else {
        this.#shrinking = 0
        this.#coasted = 0
      }
    }
    this.#wheelSize = size
    if (this.#shrinking >= COASTING && !this.#coasting) {
      this.#coasting = true
      // The momentum before it showed as such doesn't count towards a refresh.
      // The list shows it too, so it's armed only when it refreshes.
      if (this.#pull !== 0 && this.#el) {
        const pull = this.#pull - this.#coasted
        this.#pull = Math.sign(pull) === Math.sign(this.#pull) ? pull : 0
        if (animate()) this.offset = stretch(this.#pull, this.#el.clientHeight)
      }
    }
    // The fingers are back, as a new gesture.
    if (this.#coasting && this.#growing >= HAND_BACK) this.#wheelBegan()
    if (this.#coasting) {
      // The fingers lifted: a stretched list springs back at once, as it does natively,
      // and the momentum neither stretches it nor scrolls it.
      if (this.#pull !== 0 || this.#frame !== 0) {
        if (this.refreshing) return
        e.preventDefault()
        this.#bounced = true
        if (this.#pull !== 0) this.#release()
      } else if (!this.#bounced && this.#atEdge(-e.deltaY) && animate()) {
        // Momentum reached an edge: it bounces the list off it.
        this.#bounced = true
        e.preventDefault()
        this.#settle(-Math.sign(e.deltaY) * Math.min(size, BOUNCE_MAX), BOUNCE_MS, () => this.#settle(0))
      }
      return
    }
    const pull = -Math.sign(e.deltaY) * Math.min(size, WHEEL_MAX) * WHEEL_PULL
    if (!this.#take(pull)) return
    if (this.#growing === 0) this.#coasted += pull
    e.preventDefault()
    clearTimeout(this.#idle)
    this.#idle = window.setTimeout(this.#release, WHEEL_IDLE)
  }

  #onTouchStart = (e: TouchEvent) => {
    this.#touchY = e.touches.length === 1 ? e.touches[0].clientY : null
    this.#touchOwned = false
    this.#began()
  }

  #wheelBegan() {
    this.#began()
    this.#shrinking = 0
    this.#growing = 0
    this.#coasting = false
    this.#bounced = false
    this.#coasted = 0
  }

  #began() {
    // Still stretched at the top, as while springing back, counts as at the top.
    this.fromTop = this.offset > 0 || (this.#el?.scrollTop ?? 1) <= 0
  }

  #onTouchMove = (e: TouchEvent) => {
    if (this.#touchY === null || e.touches.length !== 1) return
    const y = e.touches[0].clientY
    const delta = y - this.#touchY
    this.#touchY = y
    if (this.#take(delta)) {
      if (e.cancelable) {
        e.preventDefault()
        this.#touchOwned = true
      }
    } else if (this.#touchOwned && this.#el) {
      // The browser doesn't scroll a touch that began as a pull.
      this.#el.scrollTop -= delta
      if (e.cancelable) e.preventDefault()
    }
  }

  #onTouchEnd = (e: TouchEvent) => {
    if (e.touches.length > 0) return
    this.#touchY = null
    this.#touchOwned = false
    if (this.#pull !== 0 || this.offset !== 0) this.#release()
  }
}
