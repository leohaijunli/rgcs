import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import PlanSteps from './panels/plan/PlanSteps'
import MissionProgressCard from './panels/MissionProgressCard'
import FlightCommands from './FlightCommands'
import { useMissionStore } from '../stores/mission'
import { useUiStore, VIEW_CONFIG } from '../stores/ui'
import { useMissionProgress } from '../hooks/useMissionProgress'
import { kindsBySeq } from '../mission/lineKinds'
import type { LineKind } from '../generated-types/LineKind'

/** Tone of a pattern line kind, matching the map polylines. */
const KIND_TONE: Record<LineKind, string> = {
  survey: 'text-accent',
  tie: 'text-warn',
  calibration: 'text-mag',
}

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

/** Fly panel (P4 §4.4): commands → progress → active-line list. Health and
 * flight dynamics live in the TopBar and the HUD (design rule 2), not here. */
function FlightInspector() {
  const { items, progress } = useMissionProgress()
  const lastPattern = useMissionStore((s) => s.lastPattern)
  const select = useMissionStore((s) => s.select)
  const kinds = useMemo(() => kindsBySeq(lastPattern?.lines ?? []), [lastPattern])

  return (
    <div className="space-y-2">
      <FlightCommands />
      <MissionProgressCard />
      <ActiveLineList items={items} progress={progress} kinds={kinds} select={select} />
    </div>
  )
}

function ActiveLineList({
  items,
  progress,
  kinds,
  select,
}: {
  items: ReturnType<typeof useMissionProgress>['items']
  progress: ReturnType<typeof useMissionProgress>['progress']
  kinds: Map<number, LineKind>
  select: (seq: number | null) => void
}) {
  const { t } = useTranslation()
  if (items.length === 0) return null
  return (
    <div>
      <h3 className="pt-1 text-xs uppercase tracking-wide text-muted">{t('panels.planItems')}</h3>
      <div className="mt-1 space-y-1">
        {items.map((item, idx) => {
          const kind = kinds.get(item.seq) ?? null
          const tone = kind ? KIND_TONE[kind] : 'text-ink'
          const active = idx === progress.targetIndex
          const flown = progress.targetIndex != null && idx < progress.targetIndex
          const label = kind ? t(`plan.pattern.legend.${kindLegendKey(kind)}`) : t('plan.waypoint')
          return (
            <button
              key={item.seq}
              onClick={() => select(item.seq)}
              aria-current={active ? 'step' : undefined}
              className={`flex w-full touch-target items-center gap-2 rounded border px-2 py-1.5 text-sm ${
                active
                  ? 'border-accent bg-accent/10'
                  : flown
                    ? 'border-line bg-canvas opacity-55'
                    : 'border-line bg-canvas hover:bg-panel'
              }`}
            >
              {/* Flight-state colouring (operator request): flown = green
                  check tone, target = accent (row highlight), future = muted. */}
              <span
                className={`mono w-6 shrink-0 text-right ${active ? 'text-accent' : flown ? 'text-ok' : 'text-muted'}`}
              >
                {item.seq}
              </span>
              <span className={`min-w-0 flex-1 truncate text-left ${tone}`}>{label}</span>
              <span className="mono shrink-0 text-xs text-muted">{item.altitude_m.toFixed(1)} m</span>
            </button>
          )
        })}
      </div>
    </div>
  )
}

/** Pattern-line kind to the `plan.pattern.legend.*` shortcut. */
function kindLegendKey(kind: LineKind): string {
  switch (kind) {
    case 'survey':
      return 'survey'
    case 'tie':
      return 'tie'
    case 'calibration':
      return 'cal'
  }
}