import { useTranslation } from 'react-i18next'
import PlanSteps from './panels/plan/PlanSteps'
import { useMissionStore } from '../stores/mission'
import { useUiStore, VIEW_CONFIG } from '../stores/ui'
import { useMissionProgress } from '../hooks/useMissionProgress'

export default function RightInspector() {
  const { t } = useTranslation()
  const view = useUiStore((s) => s.view)
  const rightOpen = useUiStore((s) => s.rightOpen)
  const toggleRight = useUiStore((s) => s.toggleRight)

  // Closed (default on narrow screens, §4.7): leave a slim edge toggle so the
  // panel is one tap away instead of unreachable.
  if (!rightOpen) {
    return (
      <button
        onClick={toggleRight}
        title={t('panels.openPanel')}
        aria-label={t('panels.openPanel')}
        className="flex w-6 shrink-0 items-center justify-center border-l border-line bg-panel text-muted hover:text-ink"
      >
        <span className="text-xs">◀</span>
      </button>
    )
  }

  const tab = VIEW_CONFIG[view].rightTab

  return (
    <aside className="flex w-[264px] shrink-0 flex-col border-l border-line bg-panel">
      <div className="flex items-center justify-between border-b border-line px-3 py-2">
        <h2 className="text-sm font-medium">{t(tab)}</h2>
        <button
          onClick={toggleRight}
          title={t('panels.closePanel')}
          className="touch-target rounded px-2 text-sm text-muted hover:text-ink"
        >
          ×
        </button>
      </div>
      <div className="flex-1 overflow-y-auto p-3">
        {view === 'planning' ? <PlanSteps /> : <FlightInspector />}
      </div>
    </aside>
  )
}

/** Fly panel (operator request 2026-10-11): commands + the auto-fetch
 * notice only. The waypoint list and the progress readout are gone — the
 * FC's target waypoint blinks on the map (cesium/entities.ts), progress and
 * flight dynamics live in the TopBar and the HUD. */
function FlightInspector() {
  const { items } = useMissionProgress()
  const syncState = useMissionStore((s) => s.syncState)

  return (
    <div className="space-y-2">
      {/* Auto-fetch notice: the FC is flying a mission the local (memory-only)
       * plan does not hold yet; the store fetches it on the first
       * MISSION_CURRENT and this fills in the progress. */}
      {items.length === 0 && syncState === 'downloading' && (
        <div className="rounded border border-line bg-canvas px-2 py-1 text-xs text-muted">
          Fetching the FC mission…
        </div>
      )}
    </div>
  )
}
