// The sync services the account picker offers. All speak the Google Reader API. The logos
// are their owners' marks, used to name the service, and render as masks in the text color.
import freshrss from './logos/freshrss.svg'
import miniflux from './logos/miniflux.svg'
import theOldReader from './logos/theoldreader.png'
import { t } from './i18n.svelte'

export interface Service {
  id: 'theoldreader' | 'freshrss' | 'miniflux' | 'other'
  name: string
  /** Under the name in the picker. */
  detail: string
  /** A mask image; without one, the picker shows a generic icon. */
  logo?: string
  /** The API address, for a hosted service: the form then doesn't ask for one. */
  server?: string
  /** What the service calls the user name and the password. Its help, on how to get a
   * password the API accepts, is the message `services.<id>.help`. */
  username: 'email' | 'username'
  password: 'password' | 'apiPassword'
}

export const hosted: Service[] = [
  {
    id: 'theoldreader',
    name: 'The Old Reader',
    detail: 'theoldreader.com',
    logo: theOldReader,
    server: 'https://theoldreader.com',
    username: 'email',
    password: 'password',
  },
]

export const selfHosted: Service[] = [
  {
    id: 'freshrss',
    name: 'FreshRSS',
    detail: 'freshrss.org',
    logo: freshrss,
    username: 'username',
    password: 'apiPassword',
  },
  {
    id: 'miniflux',
    name: 'Miniflux',
    detail: 'miniflux.app',
    logo: miniflux,
    username: 'username',
    password: 'password',
  },
  {
    id: 'other',
    get name() {
      return t('services.otherName')
    },
    detail: 'Google Reader API',
    username: 'username',
    password: 'apiPassword',
  },
]

/** The service an account's saved API address belongs to, to sign in to it again. */
export function serviceFor(server: string): Service {
  const all = [...hosted, ...selfHosted]
  const find = (id: Service['id']) => all.find((s) => s.id === id)!
  try {
    const url = new URL(server)
    if (url.hostname === 'theoldreader.com') return find('theoldreader')
    if (url.pathname.replace(/\/$/, '').endsWith('/api/greader.php')) return find('freshrss')
  } catch {
    // Not a URL: the generic form shows it to fix.
  }
  return find('other')
}
