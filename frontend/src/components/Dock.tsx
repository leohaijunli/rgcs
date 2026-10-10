import type { DockTab } from '../stores/ui'

/** Tabs with real content. Both current entries are Phase 3–5 placeholders, so
 * the dock is not rendered until a content tab exists (P0 C9/C10). */
const TABS: DockTab[] = []

export default function Dock() {
  if (TABS.length === 0) return null
  return null
}