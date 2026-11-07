// Single-key shortcuts (FR-10). Keys are named as `keyName` returns them: 'j', ' ',
// 'Shift+j', 'Enter'. The user can rebind every action in Settings; config.json keeps
// only the keys that differ from the defaults.
import { t } from './i18n.svelte'
import type { ShortcutAction } from './types'

/** In the order Settings lists them. */
export const shortcutActions: ShortcutAction[] = [
  'next',
  'previous',
  'readOn',
  'readBack',
  'toggleRead',
  'toggleStar',
  'openInBrowser',
  'toggleReaderView',
  'sync',
  'markAllRead',
]

const defaults: Record<ShortcutAction, string> = {
  next: 'j',
  previous: 'k',
  readOn: ' ',
  readBack: 'Shift+ ',
  toggleRead: 'm',
  toggleStar: 's',
  openInBrowser: 'b',
  toggleReaderView: 'v',
  sync: 'r',
  markAllRead: 'a',
}

/** Keys that can't be bound to anything else. ↑ and ↓ move in the sidebar or the article
 *  list, whichever is in focus; ← and → move the focus between them. */
export const arrowKeys = new Set(['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'])

type Overrides = Partial<Record<ShortcutAction, string | null>>

/** The key of each action, `null` for none. */
export function bindings(overrides: Overrides): Record<ShortcutAction, string | null> {
  const result = { ...defaults } as Record<ShortcutAction, string | null>
  for (const action of shortcutActions) {
    const key = overrides[action]
    // An arrow key saved before the arrows were reserved does nothing now: use the default.
    if (key !== undefined && !(key !== null && arrowKeys.has(key))) result[action] = key
  }
  return result
}

/**
 * Gives `action` the `key` (or no key, for `null`). An action that had the key loses
 * it. Returns the new overrides, without entries that match the defaults, and the
 * action that lost its key, if any.
 */
export function rebind(
  overrides: Overrides,
  action: ShortcutAction,
  key: string | null,
): { overrides: Overrides; displaced: ShortcutAction | null } {
  const current = bindings(overrides)
  let displaced: ShortcutAction | null = null
  if (key !== null) {
    for (const other of shortcutActions) {
      if (other !== action && current[other] === key) {
        current[other] = null
        displaced = other
      }
    }
  }
  current[action] = key
  const result: Overrides = {}
  for (const a of shortcutActions) {
    if (current[a] !== defaults[a]) result[a] = current[a]
  }
  return { overrides: result, displaced }
}

/** Keys that are never shortcuts: modifiers on their own, keys still being composed, and
 * keys the browser can't name. */
const ignoredKeys = new Set([
  'Shift', 'Control', 'Alt', 'Meta', 'CapsLock', 'Fn', 'AltGraph', 'Dead', 'Process', 'Unidentified',
])

/** The key's name as a shortcut, or `null` when it can't be one: see `ignoredKeys`, and
 * combinations with Cmd, Ctrl or Alt, which belong to the system and the menus. */
export function keyName(e: KeyboardEvent): string | null {
  if (e.metaKey || e.ctrlKey || e.altKey || e.isComposing || ignoredKeys.has(e.key)) return null
  let key = e.key
  if (key.length === 1) {
    // Letters match with Caps Lock on too. With a non-Latin layout, such as Cyrillic, a
    // letter key goes by its Latin letter, so J stays J whichever layout is on.
    const nonLatinLetter = /\p{L}/u.test(key) && !/\p{Script=Latin}/u.test(key)
    if (nonLatinLetter && /^Key[A-Z]$/.test(e.code)) key = e.code.slice(3)
    key = key.toLowerCase()
    // A shifted symbol is its own character already: '!' rather than 'Shift+1'.
    if (key !== ' ' && key.toUpperCase() === key) return key
  }
  return e.shiftKey ? `Shift+${key}` : key
}

const keyLabels: Record<string, string> = {
  ArrowUp: '↑',
  ArrowDown: '↓',
  ArrowLeft: '←',
  ArrowRight: '→',
}

/** How a key name shows in Settings: '⇧Space' on macOS, 'Shift+Space' elsewhere. */
export function keyLabel(name: string, mac: boolean): string {
  const shift = name.startsWith('Shift+')
  const key = shift ? name.slice('Shift+'.length) : name
  const label =
    key === ' '
      ? t('keys.space')
      : key === 'Enter'
        ? t(mac ? 'keys.return' : 'keys.enter')
        : (keyLabels[key] ?? (key.length === 1 ? key.toUpperCase() : key))
  if (!shift) return label
  return mac ? `⇧${label}` : `${t('keys.shift')}+${label}`
}

/** Alt+1 to Alt+9 (⌘1 to ⌘9 on macOS) switch to the first nine accounts. Not ⌥ on
 * macOS: with many layouts, such as Swedish and German, ⌥ and a digit types @, [ or |.
 * macOS lists the accounts in the Accounts menu too, with the same keys: see menu.rs. */
const accountShortcuts = 9

/** The modifier of the account shortcuts, as `aria-keyshortcuts` names it. */
export const accountModifier = (mac: boolean) => (mac ? 'Meta' : 'Alt')

/** The index of the account the key switches to, or `null`. Goes by the key's place,
 * not its character, which depends on the layout. */
export function accountIndex(e: KeyboardEvent, mac: boolean): number | null {
  const modifier = mac ? e.metaKey && !e.altKey : e.altKey && !e.metaKey
  if (!modifier || e.ctrlKey || e.shiftKey) return null
  const digit = /^Digit([1-9])$/.exec(e.code)
  return digit ? Number(digit[1]) - 1 : null
}

/** How the shortcut of the account at `index` shows: '⌘1' on macOS, 'Alt+1' elsewhere. */
export function accountShortcut(index: number, mac: boolean): string | null {
  if (index >= accountShortcuts) return null
  return mac ? `⌘${index + 1}` : `${t('keys.alt')}+${index + 1}`
}
