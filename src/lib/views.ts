import CircleDot from '@lucide/svelte/icons/circle-dot'
import Layers from '@lucide/svelte/icons/layers'
import Star from '@lucide/svelte/icons/star'
import type { View } from './types'

export interface ViewInfo {
  view: View
  label: string
  /** The sidebar row and list title for every feed at once. */
  allLabel: string
  /** What an empty list says. */
  empty: string
  icon: typeof Star
}

/** In the order of the sidebar's button group. */
export const views: ViewInfo[] = [
  { view: 'unread', label: 'Unread', allLabel: 'All Unread', empty: 'No Unread Articles', icon: CircleDot },
  { view: 'starred', label: 'Starred', allLabel: 'All Starred', empty: 'No Starred Articles', icon: Star },
  { view: 'all', label: 'All Articles', allLabel: 'All Articles', empty: 'No Articles', icon: Layers },
]

export const viewInfo = (view: View): ViewInfo => views.find((v) => v.view === view)!
