// Pane layout, kept in localStorage until there is a settings store.
const KEY = 'skvaller.layout'

export const SIDEBAR = { min: 180, max: 360, default: 230 }
export const LIST = { min: 260, max: 560, default: 340 }

interface Layout {
  sidebarOpen: boolean
  sidebarWidth: number
  listWidth: number
}

const clamp = (n: unknown, r: { min: number; max: number; default: number }) =>
  typeof n === 'number' && Number.isFinite(n) ? Math.min(r.max, Math.max(r.min, n)) : r.default

function load(): Layout {
  let saved: Partial<Layout> = {}
  try {
    saved = JSON.parse(localStorage.getItem(KEY) ?? '{}')
  } catch {
    // Storage blocked or corrupt: use defaults.
  }
  return {
    sidebarOpen: saved.sidebarOpen !== false,
    sidebarWidth: clamp(saved.sidebarWidth, SIDEBAR),
    listWidth: clamp(saved.listWidth, LIST),
  }
}

export const layout: Layout = $state(load())

export function saveLayout() {
  try {
    localStorage.setItem(KEY, JSON.stringify(layout))
  } catch {
    // Not persisting is fine.
  }
}

export function toggleSidebar() {
  layout.sidebarOpen = !layout.sidebarOpen
  saveLayout()
}
