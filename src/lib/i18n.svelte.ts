// The UI's text in the user's language. Each language is a flat JSON file in
// src/locales, named by its BCP 47 tag (`sv.json`, `pt-BR.json`); en.json is the source,
// and its keys are the only valid ones. A message missing from a language, or empty,
// shows in English. See CONTRIBUTING.md for adding a language.
//
// Plurals are keys with the plural category as their ending, as i18next names them:
// `articles_one`, `articles_other` and, for languages with more forms, `_few` and the
// rest. `t('articles', { count })` picks the form `Intl.PluralRules` gives for `count`.
import en from '../locales/en.json'

type Catalog = Record<string, string>
type Category = Intl.LDMLPluralRule
type EnglishKey = keyof typeof en
type PluralBase<K> = K extends `${infer Base}_other` ? Base : never

type PluralKey = PluralBase<EnglishKey>

/** A message's key, as `t` takes it: a plural's without its category ending. */
export type MessageKey = Exclude<EnglishKey, `${string}_${Category}`> | PluralKey

type Params = Record<string, string | number>

/** `t`'s parameters after the key: a plural message needs `count`. */
export type ParamsFor<K extends MessageKey> = K extends PluralKey ? [params: Params & { count: number }] : [params?: Params]

const catalogs: Record<string, Catalog> = Object.fromEntries(
  Object.entries(import.meta.glob<Catalog>('../locales/*.json', { eager: true, import: 'default' })).map(
    ([path, catalog]) => [path.slice(path.lastIndexOf('/') + 1, -'.json'.length), catalog],
  ),
)

/** The languages there are messages for, by tag, English first. */
export const languages = ['en', ...Object.keys(catalogs).filter((tag) => tag !== 'en').sort()]

/** The language's name in itself, capitalized as in a list: "Svenska" for `sv`. */
export function languageName(tag: string): string {
  try {
    const name = new Intl.DisplayNames([tag], { type: 'language' }).of(tag) ?? tag
    return name.charAt(0).toLocaleUpperCase(tag) + name.slice(1)
  } catch {
    return tag
  }
}

/** The language and its likely script, such as `zh-Hans` for `zh-CN` and `de-Latn` for
 * `de-AT`, so a region's variant finds its language without crossing scripts. */
function languageAndScript(tag: string): string {
  try {
    const locale = new Intl.Locale(tag).maximize()
    return `${locale.language}-${locale.script}`
  } catch {
    return tag.toLowerCase()
  }
}

const byLowercase = new Map(languages.map((l) => [l.toLowerCase(), l]))
// The first wins: `pt` before `pt-BR`, as `languages` is sorted.
const byLanguageAndScript = new Map<string, string>()
for (const l of languages) if (!byLanguageAndScript.has(languageAndScript(l))) byLanguageAndScript.set(languageAndScript(l), l)

/** The first wanted language there are messages for, as asked (`pt-BR`) or in the same
 * language and script (`de` for `de-AT`, `zh-Hant` for `zh-TW`). English if none. */
function resolve(wanted: readonly string[]): string {
  for (const tag of wanted) {
    const match = byLowercase.get(tag.toLowerCase()) ?? byLanguageAndScript.get(languageAndScript(tag))
    if (match) return match
  }
  return 'en'
}

/** The system's languages, most wanted first. Rust passes them (see locale.rs):
 * WKWebView's `navigator.languages` lists the app bundle's languages, not the user's. */
function systemLanguages(): readonly string[] {
  const tags = (window as { __SKVALLER_SYSTEM_LANGUAGES__?: unknown }).__SKVALLER_SYSTEM_LANGUAGES__
  return Array.isArray(tags) && tags.length > 0 ? tags.filter((t) => typeof t === 'string') : navigator.languages
}

let locale = $state(resolve(systemLanguages()))

/** The language the UI shows, by tag. Reactive. */
export const currentLanguage = () => locale

/** Shows the UI in the language of the `language` setting; `null`, or a language
 * there are no messages for any more, follows the system. */
export function setLanguage(language: string | null) {
  locale = resolve(language ? [language, ...systemLanguages()] : systemLanguages())
  document.documentElement.lang = locale
}

const pluralRules = new Map<string, Intl.PluralRules>()
/** The system's, as for dates: numbers follow the region, not the UI's language. */
const numbers = new Intl.NumberFormat()

function lookup(tag: string, key: string, count: number | undefined): string | undefined {
  const catalog = catalogs[tag]
  if (count === undefined) return catalog[key] || undefined
  let rules = pluralRules.get(tag)
  if (!rules) pluralRules.set(tag, (rules = new Intl.PluralRules(tag)))
  const category = rules.select(count)
  return catalog[`${key}_${category}`] || catalog[`${key}_other`] || undefined
}

/**
 * The message for `key` in the UI's language, with each `{name}` replaced by
 * `params.name`. Numbers are formatted as the system formats them. A plural message needs
 * `params.count`. Reactive: the text follows the language.
 */
export function t<K extends MessageKey>(key: K, ...[params]: ParamsFor<K>): string {
  const count = `${key}_other` in en && typeof params?.count === 'number' ? params.count : undefined
  const message = lookup(locale, key, count) ?? lookup('en', key, count) ?? key
  if (!params) return message
  return message.replace(/\{(\w+)\}/g, (placeholder, name: string) => {
    const value = params[name]
    if (value === undefined) return placeholder
    return typeof value === 'number' ? numbers.format(value) : value
  })
}
