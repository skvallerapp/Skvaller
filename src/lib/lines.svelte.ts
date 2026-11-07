// Counts the lines a text wraps to, from canvas measurements instead of DOM layout, so
// the article list can know its rows' heights before drawing them.

const context = document.createElement('canvas').getContext('2d')!

/** Bumped when fonts finish loading, since text measured before then used a fallback. */
let fontsLoaded = $state(0)

/** The font everything cached was measured in. Setting `context.font` is slow, so it's
 *  set only when this changes. */
let font = ''
let space = 0
/** Widths of words and pieces of words. */
const widths = new Map<string, number>()
/** Each text cut where CSS may wrap it, after spaces and hyphens: each piece's width, and
 *  whether a space comes before it. */
const texts = new Map<string, { widths: Float64Array; spaced: Uint8Array }>()

function useFont(f: string) {
  if (f === font) return
  font = context.font = f
  widths.clear()
  texts.clear()
  space = measure(' ')
}

// WebKit fires no `loadingdone` for fonts that load from CSS, so the font is asked for
// what new pieces need, all at once, and everything is measured again once it has loaded.
// Each piece is checked before it's measured: measuring starts a load in WebKit, and then
// `check` says the font is ready although it isn't.
let missing = ''
/** Fonts and pieces already asked for, so a font that can't load is asked only once. */
const requested = new Set<string>()

function ensureLoaded(piece: string) {
  const key = `${font}\n${piece}`
  if (requested.has(key) || document.fonts.check(font, piece)) return
  requested.add(key)
  if (missing === '') queueMicrotask(loadMissing)
  missing += piece
}

function loadMissing() {
  const [f, text] = [font, missing]
  missing = ''
  document.fonts.load(f, text).then(
    () => {
      if (f !== font) return
      font = ''
      fontsLoaded++
    },
    () => {},
  )
}

function measure(piece: string) {
  let w = widths.get(piece)
  if (w === undefined) {
    ensureLoaded(piece)
    w = context.measureText(piece).width
    widths.set(piece, w)
  }
  return w
}

// Text without spaces, as in Chinese and Japanese, wraps between any two characters,
// except before closing punctuation and small kana, or after opening punctuation.
const cjk = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}\u3000-\u303f\uff00-\uffef]/u
const noStart = /[、。，．,.：；:;？！?!）)」』】〕〉》”’ー…‥ぁぃぅぇぉっゃゅょゎゕゖァィゥェォッャュョヮヵヶ・%％]/
const noEnd = /[（(「『【〔〈《“‘]/

/** Whether a line may break between characters `a` and `b`. */
function breaks(a: string, b: string) {
  if (a === '-') return b !== '-'
  return (cjk.test(a) || cjk.test(b)) && !noStart.test(b) && !noEnd.test(a)
}

function piecesOf(text: string) {
  let p = texts.get(text)
  if (p) return p
  const w: number[] = []
  const spaced: number[] = []
  const push = (piece: string, space: boolean) => {
    w.push(measure(piece))
    spaced.push(space ? 1 : 0)
  }
  for (const word of text.split(/\s+/)) {
    if (word === '') continue
    // A word's first piece follows a space, unless it starts the text.
    let space = w.length > 0
    let piece = ''
    let prev = ''
    for (const c of word) {
      if (piece !== '' && breaks(prev, c)) {
        push(piece, space)
        space = false
        piece = ''
      }
      piece += c
      prev = c
    }
    push(piece, space)
  }
  p = { widths: Float64Array.from(w), spaced: Uint8Array.from(spaced) }
  texts.set(text, p)
  return p
}

/**
 * The number of lines, at most `max`, that `text` wraps to at `width` in font `f` (a CSS
 * font shorthand). Reactive to fonts loading.
 *
 * Canvas and layout widths can differ by a fraction of a pixel, so a line is taken as full
 * a pixel early: a row may get a line too many, never too few.
 */
export function lineCount(text: string, f: string, width: number, max: number): number {
  void fontsLoaded
  useFont(f)
  const { widths, spaced } = piecesOf(text)
  const room = width - 1
  let lines = 1
  let x = 0
  for (let i = 0; i < widths.length; i++) {
    const w = widths[i]
    const gap = spaced[i] ? space : 0
    if (x > 0 && x + gap + w > room) {
      if (++lines > max) return max
      x = 0
    }
    x += x > 0 ? gap + w : w
    // A piece wider than the line, such as a long URL: CSS doesn't break it, but counting
    // the lines it would take is the safe side.
    while (x > room && room > 0) {
      if (++lines > max) return max
      x -= room
    }
  }
  return lines
}
