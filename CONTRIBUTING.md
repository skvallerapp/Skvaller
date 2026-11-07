# Contributing

## Translations

Skvaller's text lives in [`src/locales`](src/locales), one JSON file per language.
[`en.json`](src/locales/en.json) is the source: every other language has the same keys,
with its own text.

### Add a language

1. Copy `src/locales/en.json` to `src/locales/<tag>.json`, where `<tag>` is the language's
   [BCP 47 tag](https://en.wikipedia.org/wiki/IETF_language_tag): `sv` for Swedish,
   `de` for German, `pt-BR` for Brazilian Portuguese. Use the bare language (`de`) unless
   the variants differ enough to need their own files. A user with `de-AT` gets `de`.
   For Chinese, use `zh-Hans` (Simplified) and `zh-Hant` (Traditional).
2. Translate the values. Leave the keys as they are.
3. Run `bun run i18n:check <tag>`. It fails on mistakes, and lists the messages still
   missing or still the same as in English. Some may rightly stay the same.
4. Open a pull request.

You don't have to translate everything at once. A missing or empty message shows in
English. Once the app has a second language, Settings → Appearance gets a Language
setting. The app follows the system's language until the user picks another one.

To see your translation, run `bun run dev` and open `http://localhost:1420` in a browser
set to your language, or pick it in Settings at `http://localhost:1420/settings.html`.
The browser version uses demo data.

### Placeholders

Text in braces, such as `{account}` in `"Remove {account}?"`, is filled in by the app.
Keep each placeholder exactly as it is, and put it where your language needs it.

### Plurals

A message that depends on a number has one key for each plural form, named
`<key>_<form>`. English has two forms:

```json
"list.unread_one": "{count} Unread Item",
"list.unread_other": "{count} Unread Items",
```

Your language may have different forms:

- Polish uses `_one`, `_few`, `_many` and `_other`.
- Japanese, Chinese and Korean use only `_other`: delete the `_one` keys.
- French, Spanish, Italian and Portuguese have `_many`, for millions.

`bun run i18n:check` lists the forms your language needs, and fails on forms it doesn't
use. The
[CLDR plural rules](https://www.unicode.org/cldr/charts/latest/supplemental/language_plural_rules.html)
explain which numbers take each form. Keep `{count}` in every form: in many languages,
`_one` is for other numbers too, such as 21 in Russian and 0 in French.

### Context

Some short messages need context:

| Key | Context |
| --- | --- |
| `shortcuts.actions.readOn` | Scrolls the article a page down, or opens the next one at its end. |
| `shortcuts.actions.readBack` | Scrolls the article a page up. |
| `keys.*` | Names of keys, as printed on the keyboard. |
| `sidebar.viewPicker` | Names the buttons that pick Unread, Starred or All Articles. |
| `settings.accountName` | The account's name, which the user can change. |
| `settings.syncSchedule` | Labels the choice of how often to sync. |
| `settings.clocks.*` | A 24- or 12-hour clock, not a length of time. |
| `common.twoSentences` | Two sentences shown together. Change the space if your language uses none. |

### Style

English labels use title case (“Mark All as Read”). Follow your own language's
conventions. For example, German and Swedish use sentence case. Dates, times and numbers
follow the system's region settings, not the language setting, so they don't need
translating.

### For developers

The UI never has English text in its code. To add or change text:

- Add the key to `en.json` only. Name it `<area>.<name>`, such as `sidebar.sync`.
  A key can't also be the start of another key: `settings.theme` and
  `settings.theme.dark` can't both exist.
- Show it with `t('area.name')`, or `t('area.name', { count, name })`, from
  `src/lib/i18n.svelte.ts`. Keys are type-checked: `bun run check` fails on unknown
  ones.
- `t` is reactive. Call it where the text renders (in markup, `$derived`, or a getter),
  not once at module load, or the text won't follow a change of language.
- Don't build sentences from pieces. Use one message with placeholders, so a translator
  can change the word order.
- When the meaning of an English message changes, give it a new key. The old
  translations then fall back to English instead of saying something else.
