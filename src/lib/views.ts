import Layers from '@lucide/svelte/icons/layers'
import Star from '@lucide/svelte/icons/star'
import type { Component } from 'svelte'
import { t } from './i18n.svelte'
import type { View } from './types'
import UnreadIcon from './UnreadIcon.svelte'

/** A Lucide icon, or one drawn to the same size. */
export type ViewIcon = Component<{ size?: number; strokeWidth?: number }>

/** The labels are in the UI's language. */
export interface ViewInfo {
  view: View
  readonly label: string
  /** The sidebar row and list title for every feed at once. */
  readonly allLabel: string
  /** What an empty list says. */
  readonly empty: string
  icon: ViewIcon
}

const info = (view: View, icon: ViewIcon): ViewInfo => ({
  view,
  icon,
  get label() {
    return t(`views.${view}.label`)
  },
  get allLabel() {
    return t(`views.${view}.all`)
  },
  get empty() {
    return t(`views.${view}.empty`)
  },
})

/** In the order of the sidebar's button group. */
export const views: ViewInfo[] = [info('unread', UnreadIcon), info('starred', Star), info('all', Layers)]

export const viewInfo = (view: View): ViewInfo => views.find((v) => v.view === view)!
