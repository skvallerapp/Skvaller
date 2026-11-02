import Layers from '@lucide/svelte/icons/layers'
import Star from '@lucide/svelte/icons/star'
import type { Component } from 'svelte'
import type { View } from './types'
import UnreadIcon from './UnreadIcon.svelte'

/** A Lucide icon, or one drawn to the same size. */
export type ViewIcon = Component<{ size?: number; strokeWidth?: number }>

export interface ViewInfo {
  view: View
  label: string
  /** The sidebar row and list title for every feed at once. */
  allLabel: string
  /** What an empty list says. */
  empty: string
  icon: ViewIcon
}

/** In the order of the sidebar's button group. */
export const views: ViewInfo[] = [
  { view: 'unread', label: 'Unread', allLabel: 'All Unread', empty: 'No Unread Articles', icon: UnreadIcon },
  { view: 'starred', label: 'Starred', allLabel: 'All Starred', empty: 'No Starred Articles', icon: Star },
  { view: 'all', label: 'All Articles', allLabel: 'All Articles', empty: 'No Articles', icon: Layers },
]

export const viewInfo = (view: View): ViewInfo => views.find((v) => v.view === view)!
