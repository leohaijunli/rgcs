import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import PlanSteps from './panels/plan/PlanSteps'
import PreparePanel from './panels/prepare/PreparePanel'
import PreflightStagePanel from './panels/preflight/PreflightStagePanel'
import ReviewPanel from './panels/review/ReviewPanel'
import DrapePanel from './panels/fly/DrapePanel'
import { useDrapeMonitorStore } from '../stores/drapeMonitor'
import { useUiStore, VIEW_CONFIG } from '../stores/ui'
import { IconButton } from '../design-system/components'

/** The right-panel content per stage; `fly` keeps the map + HUD (its
 * telemetry surfaces are the HUD and the QC dock, not this panel). */
function StagePanel({ view }: { view: ReturnType<typeof useUiStore.getState>['view'] }) {
  switch (view) {
    case 'prepare':
      return <PreparePanel />
    case 'plan':
      return <PlanSteps />
    case 'preflight':
      return <PreflightStagePanel />
    case 'fly':
      return (
        <div className="space-y-2">
          <DrapePanel />
        </div>
      )
    case 'review':
      return <ReviewPanel />
  }
}

export default function RightInspector() {
  const { t } = useTranslation()
  const view = useUiStore((s) => s.view)
  const rightOpen = useUiStore((s) => s.rightOpen)
  const toggleRight = useUiStore((s) => s.toggleRight)
  // The monitor lives exactly as long as the Fly stage does (DR6): started
  // when a verified plan is flying, stopped when the operator navigates away.
  const ensureMonitor = useDrapeMonitorStore((s) => s.ensure)
  const stopMonitor = useDrapeMonitorStore((s) => s.stop)
  useEffect(() => {
    if (view === 'fly') void ensureMonitor()
    else void stopMonitor()
  }, [view, ensureMonitor, stopMonitor])

  // Closed (default on narrow screens, §4.7): leave a slim edge toggle so the
  // panel is one tap away instead of unreachable. Full-height 24px strip, not
  // the square IconButton shape — kept bespoke on purpose (UI3).
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
    <aside className="flex w-[288px] shrink-0 flex-col border-l border-line bg-panel">
      <div className="flex items-center justify-between border-b border-line px-3 py-2">
        <h2 className="text-sm font-medium">{t(tab)}</h2>
        <IconButton title={t('panels.closePanel')} size={28} onClick={toggleRight}>
          <span className="text-sm">×</span>
        </IconButton>
      </div>
      <div className="flex-1 overflow-y-auto p-3">
        <StagePanel view={view} />
      </div>
    </aside>
  )
}

