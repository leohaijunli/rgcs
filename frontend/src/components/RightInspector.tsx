import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import PlanSteps from './panels/plan/PlanSteps'
import MissionProgressCard from './panels/MissionProgressCard'
import { useLinkStore } from '../stores/link'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore, VIEW_CONFIG } from '../stores/ui'
import { useMissionStore } from '../stores/mission'
import { useMissionProgress } from '../hooks/useMissionProgress'
import { kindsBySeq } from '../mission/lineKinds'
import { modeLabel } from '../util/modeLabel'
import type { LineKind } from '../generated-types/LineKind'
import type { LinkStatus } from '../generated-types/LinkStatus'
import type { Heartbeat } from '../generated-types/Heartbeat'

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

  if (!rightOpen) return null

  const tab = VIEW_CONFIG[view].rightTab

  return (
    <aside className="flex w-[264px] shrink-0 flex-col border-l border-line bg-panel">
      <div className="flex items-center justify-between border-b border-line px-3 py-2">
        <h2 className="text-sm font-medium">{t(tab)}</h2>
        <button
          onClick={toggleRight}
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

/** Fly view: mission progress + the active-line list (the Vehicles drawer's
 * flight section moved here when the drawer was removed, P1). */
function FlightInspector() {
  const link = useLinkStore((s) => s.link)
  const heartbeat = useTelemetryStore((s) => s.snapshot?.heartbeat ?? null)
  const { items, progress } = useMissionProgress()
  const lastPattern = useMissionStore((s) => s.lastPattern)
  const focus = useMissionStore((s) => s.focus)
  const kinds = useMemo(() => kindsBySeq(lastPattern?.lines ?? []), [lastPattern])

  return (
    <div className="space-y-2">
      <MissionProgressCard />
      <ActiveLineList items={items} progress={progress} kinds={kinds} focus={focus} />
      <StatusCard link={link} heartbeat={heartbeat} />
    </div>
  )
}

function ActiveLineList({
  items,
  progress,
  kinds,
  focus,
}: {
  items: ReturnType<typeof useMissionProgress>['items']
  progress: ReturnType<typeof useMissionProgress>['progress']
  kinds: Map<number, LineKind>
  focus: (seq: number) => void
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
          const label = kind ? t(`plan.pattern.legend.${kindLegendKey(kind)}`) : t('plan.waypoint')
          return (
            <button
              key={item.seq}
              onClick={() => focus(item.seq)}
              aria-current={active ? 'step' : undefined}
              className={`flex w-full touch-target items-center gap-2 rounded border px-2 py-1.5 text-sm ${
                active ? 'border-accent bg-accent/10' : 'border-line bg-canvas hover:bg-panel'
              }`}
            >
              <span className="mono w-6 shrink-0 text-right text-muted">{item.seq}</span>
              <span className={`min-w-0 flex-1 truncate text-left ${tone}`}>{label}</span>
              <span className="mono shrink-0 text-xs text-muted">{item.altitude_m.toFixed(1)} m</span>
            </button>
          )
        })}
      </div>
    </div>
  )
}

function StatusCard({
  link,
  heartbeat,
}: {
  link: LinkStatus | null
  heartbeat: Heartbeat | null
}) {
  const { t } = useTranslation()
  return (
    <div className="panel rounded p-3">
      <div className="mb-2 text-xs uppercase tracking-wide text-muted">{t('panels.status')}</div>
      <div className="space-y-1.5">
        <div className="flex items-baseline justify-between gap-3 text-sm">
          <span className="text-muted">{t('link.mode')}</span>
          <span className="mono">{modeLabel(t, heartbeat) ?? '—'}</span>
        </div>
        <div className="flex items-baseline justify-between gap-3 text-sm">
          <span className="text-muted">{t('link.armed')}</span>
          <span className="mono">
            {heartbeat?.base_mode.safety_armed ? t('link.armed') : t('link.disarmed')}
          </span>
        </div>
        <div className="flex items-baseline justify-between gap-3 text-sm">
          <span className="text-muted">{t('link.fc')}</span>
          <span className="mono">{link?.fc_alive ? t('link.connected') : t('link.lost')}</span>
        </div>
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