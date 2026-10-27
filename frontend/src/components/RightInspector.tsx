import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import PlanSteps from './panels/plan/PlanSteps'
import PreparePanel from './panels/prepare/PreparePanel'
import PreflightStagePanel from './panels/preflight/PreflightStagePanel'
import ReviewPanel from './panels/review/ReviewPanel'
import DrapePanel from './panels/fly/DrapePanel'
import LogsPanels from './panels/fly/LogsPanels'
import { useDrapeMonitorStore } from '../stores/drapeMonitor'
import { clampRightWidth, useUiStore, VIEW_CONFIG, type FlyRightTab } from '../stores/ui'
import { IconButton } from '../design-system/components'

/** The right-panel content per stage. Fly has its own Logs/Drape dock below. */
function StagePanel({ view }: { view: Exclude<ReturnType<typeof useUiStore.getState>['view'], 'fly'> }) {
  switch (view) {
    case 'prepare':
      return <PreparePanel />
    case 'plan':
      return <PlanSteps />
    case 'preflight':
      return <PreflightStagePanel />
    case 'review':
      return <ReviewPanel />
  }
}

/** Fly-stage right column: Logs/Console and the drape monitor share one
 * resizable column (the old Telemetry column is gone). */
function FlyDock({ tab, onTab }: { tab: FlyRightTab; onTab: (tab: FlyRightTab) => void }) {
  const { t } = useTranslation()
  const tabs: Array<{ id: FlyRightTab; label: string }> = [
    { id: 'logs', label: t('panels.logs') },
    { id: 'drape', label: t('panels.drape') },
  ]
  return (
    <div className="min-w-0">
      <div role="tablist" aria-label={t('panels.logs')} className="mb-3 flex gap-1">
        {tabs.map((entry) => (
          <button
            key={entry.id}
            role="tab"
            aria-selected={tab === entry.id}
            data-testid={`right-dock-tab-${entry.id}`}
            onClick={() => onTab(entry.id)}
            className={`h-8 rounded border px-2.5 text-xs ${
              tab === entry.id
                ? 'border-accent bg-canvas text-ink'
                : 'border-line bg-canvas text-muted hover:text-ink'
            }`}
          >
            {entry.label}
          </button>
        ))}
      </div>
      <div role="tabpanel">
        {tab === 'logs' ? <LogsPanels /> : <DrapePanel />}
      </div>
    </div>
  )
}

export default function RightInspector() {
  const { t } = useTranslation()
  const view = useUiStore((s) => s.view)
  const rightOpen = useUiStore((s) => s.rightOpen)
  const rightWidth = useUiStore((s) => s.rightWidth)
  const flyRightTab = useUiStore((s) => s.flyRightTab)
  const setFlyRightTab = useUiStore((s) => s.setFlyRightTab)
  const setRightWidth = useUiStore((s) => s.setRightWidth)
  const toggleRight = useUiStore((s) => s.toggleRight)
  const [dragWidth, setDragWidth] = useState<number | null>(null)
  const widthRef = useRef(rightWidth)
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
  const width = dragWidth ?? rightWidth

  // Horizontal drag on the left edge: dragging left widens the column. The
  // store write (and localStorage persist) happens once on pointer-up.
  const onResizeDown = (event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault()
    const grip = event.currentTarget
    grip.setPointerCapture(event.pointerId)
    const startX = event.clientX
    const startWidth = width
    widthRef.current = startWidth
    const onMove = (move: PointerEvent) => {
      const next = clampRightWidth(startWidth - (move.clientX - startX))
      widthRef.current = next
      setDragWidth(next)
    }
    const onUp = (up: PointerEvent) => {
      grip.releasePointerCapture(up.pointerId)
      grip.removeEventListener('pointermove', onMove)
      grip.removeEventListener('pointerup', onUp)
      setRightWidth(widthRef.current)
      setDragWidth(null)
    }
    grip.addEventListener('pointermove', onMove)
    grip.addEventListener('pointerup', onUp)
  }

  return (
    <aside
      data-testid="right-dock"
      className="relative flex shrink-0 flex-col border-l border-line bg-panel"
      style={{ width }}
    >
      <div
        data-testid="right-dock-resize"
        role="separator"
        aria-orientation="vertical"
        aria-label={t('panels.resize')}
        title={t('panels.resize')}
        onPointerDown={onResizeDown}
        className="absolute inset-y-0 left-0 z-10 w-1.5 -translate-x-1/2 cursor-ew-resize hover:bg-accent/40"
      />
      <div className="flex items-center justify-between border-b border-line px-3 py-2">
        <h2 className="text-sm font-medium">
          {view === 'fly' ? t(flyRightTab === 'logs' ? 'panels.logs' : 'panels.drape') : t(tab)}
        </h2>
        <IconButton
          title={t('panels.closePanel')}
          size={28}
          data-testid="right-dock-close"
          onClick={toggleRight}
        >
          <span className="text-sm">×</span>
        </IconButton>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        {view === 'fly' ? (
          <FlyDock tab={flyRightTab} onTab={setFlyRightTab} />
        ) : (
          <StagePanel view={view} />
        )}
      </div>
    </aside>
  )
}
