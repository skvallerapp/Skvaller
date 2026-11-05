// The sync services the account picker offers. All speak the Google Reader API. The logos
// are their owners' marks, used to name the service, and render as masks in the text color.
import freshrss from './logos/freshrss.svg'
import miniflux from './logos/miniflux.svg'
import theOldReader from './logos/theoldreader.png'

export interface Service {
  id: 'theoldreader' | 'freshrss' | 'miniflux' | 'other'
  name: string
  /** Under the name in the picker. */
  detail: string
  /** A mask image; without one, the picker shows a generic icon. */
  logo?: string
  /** The API address, for a hosted service: the form then doesn't ask for one. */
  server?: string
  username: string
  password: string
  /** How to get a password the API accepts. */
  help: string
}

export const hosted: Service[] = [
  {
    id: 'theoldreader',
    name: 'The Old Reader',
    detail: 'theoldreader.com',
    logo: theOldReader,
    server: 'https://theoldreader.com',
    username: 'Email',
    password: 'Password',
    help: 'If you sign in to The Old Reader with Google or Facebook, set a password in its settings first.',
  },
]

export const selfHosted: Service[] = [
  {
    id: 'freshrss',
    name: 'FreshRSS',
    detail: 'freshrss.org',
    logo: freshrss,
    username: 'Username',
    password: 'API password',
    help: 'In FreshRSS, allow API access under Settings → Authentication, then set an API password under Profile.',
  },
  {
    id: 'miniflux',
    name: 'Miniflux',
    detail: 'miniflux.app',
    logo: miniflux,
    username: 'Username',
    password: 'Password',
    help: 'In Miniflux, turn on the Google Reader API under Settings → Integrations. It has its own username and password.',
  },
  {
    id: 'other',
    name: 'Other',
    detail: 'Google Reader API',
    username: 'Username',
    password: 'API password',
    help: 'Any service that speaks the Google Reader API. Use its API password if it has one.',
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
