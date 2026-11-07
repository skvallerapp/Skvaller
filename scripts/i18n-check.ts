// Checks the translations in src/locales against en.json: `bun run i18n:check`, or
// `bun run i18n:check sv` for one language. Fails on mistakes: keys en.json doesn't
// have, plural forms the language doesn't use, placeholders that differ, and values
// that aren't text. Lists, without failing, the messages a language is missing, which
// show in English, and those still the same as in English.
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

const dir = join(import.meta.dirname, '../src/locales')
const categories = ['zero', 'one', 'two', 'few', 'many', 'other']
const pluralEnding = new RegExp(`_(${categories.join('|')})$`)

function read(tag: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(dir, `${tag}.json`), 'utf8'))
}

const placeholders = (message: string) => new Set([...message.matchAll(/\{(\w+)\}/g)].map((m) => m[1]))

const en = read('en') as Record<string, string>
const errors: string[] = []

// A key that is the start of another, such as `a` and `a.b`, can't be nested, as some
// translation tools store keys.
const keys = Object.keys(en)
for (const key of keys)
  for (const other of keys) if (other.startsWith(`${key}.`)) errors.push(`en: "${key}" is the start of "${other}"`)

/** The plural messages, by key without the ending. */
const plurals = new Set(keys.filter((k) => k.endsWith('_other')).map((k) => k.slice(0, -'_other'.length)))
for (const key of keys) {
  const base = key.replace(pluralEnding, '')
  if (base !== key && !plurals.has(base)) errors.push(`en: "${key}" has no "${base}_other"`)
}

const wanted = process.argv.slice(2)
const tags = readdirSync(dir)
  .filter((f) => f.endsWith('.json'))
  .map((f) => f.slice(0, -'.json'.length))
  .filter((tag) => tag !== 'en' && (wanted.length === 0 || wanted.includes(tag)))
for (const tag of wanted)
  if (tag !== 'en' && !tags.includes(tag)) errors.push(`${tag}: there is no src/locales/${tag}.json`)

for (const tag of tags) {
  try {
    if (Intl.getCanonicalLocales(tag)[0] !== tag) errors.push(`${tag}: name the file ${Intl.getCanonicalLocales(tag)[0]}.json`)
  } catch {
    errors.push(`${tag}: not a language tag, such as sv or pt-BR`)
    continue
  }
  let catalog: Record<string, unknown>
  try {
    catalog = read(tag)
  } catch (e) {
    errors.push(`${tag}: ${(e as Error).message}`)
    continue
  }
  if (typeof catalog !== 'object' || catalog === null || Array.isArray(catalog)) {
    errors.push(`${tag}: not a JSON object of keys and messages, as en.json is`)
    continue
  }
  const needed = new Intl.PluralRules(tag).resolvedOptions().pluralCategories as string[]
  const missing: string[] = []
  const untranslated: string[] = []

  for (const key of keys) {
    const base = key.replace(pluralEnding, '')
    if (base !== key) continue
    if (!catalog[key]) missing.push(key)
  }
  for (const base of plurals)
    for (const category of needed) if (!catalog[`${base}_${category}`]) missing.push(`${base}_${category}`)

  for (const [key, value] of Object.entries(catalog)) {
    const base = key.replace(pluralEnding, '')
    const plural = base !== key && plurals.has(base)
    const source = plural ? (en[key] ?? en[`${base}_other`]) : en[key]
    if (source === undefined) {
      errors.push(`${tag}: "${key}" is not in en.json`)
      continue
    }
    if (plural && !needed.includes(key.slice(base.length + 1))) {
      errors.push(`${tag}: "${key}": ${tag} has no _${key.slice(base.length + 1)} form; delete this key`)
      continue
    }
    if (typeof value !== 'string') {
      errors.push(`${tag}: "${key}" is not text`)
      continue
    }
    if (!value) continue
    // A message of placeholders alone, such as "{first} {second}", often stays as it is.
    if (value === source && /\p{L}/u.test(source.replace(/\{\w+\}/g, ''))) untranslated.push(key)
    // Every plural form keeps {count}: in many languages, the `one` form is for other
    // numbers too, such as 21 in Russian and 0 in French.
    const expected = placeholders(source)
    const found = placeholders(value)
    for (const name of found) if (!expected.has(name)) errors.push(`${tag}: "${key}" has {${name}}, which en.json doesn't`)
    for (const name of expected) if (!found.has(name)) errors.push(`${tag}: "${key}" is missing {${name}}`)
  }

  const total = keys.filter((k) => !pluralEnding.test(k)).length + plurals.size * needed.length
  console.log(`${tag}: ${total - missing.length - untranslated.length} of ${total} messages translated`)
  for (const key of missing) console.log(`  missing: ${key}`)
  // Some are rightly the same, such as a word both languages share.
  for (const key of untranslated) console.log(`  same as English: ${key}`)
}

for (const error of errors) console.error(`error: ${error}`)
if (errors.length > 0) process.exit(1)
if (tags.length === 0) console.log('en.json is fine.')
