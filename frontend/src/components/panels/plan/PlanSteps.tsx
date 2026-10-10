// Plan view, right panel (P2 §4.2): a header that is always visible (altitude
// mode + one sync badge) above four collapsible step cards, exactly one
// expanded at a time. The step order is the workflow order: area → pattern →
// waypoints → sync.

import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { planSyncStatus, type PlanSyncStatus } from '../../../mission/sync'
import { useMissionStore, type AltitudeMode } from '../../../stores/mission'
import { useUiStore, type PlanStep } from '../../../stores/ui'
import AreaStep from './AreaStep'
import PatternStep, { type PatternKind } from './PatternStep'
import WaypointsStep from './WaypointsStep'
import SyncStep from './SyncStep'
import type { CloverleafPattern } from '../../../generated-types/CloverleafPattern'
import type { SurveyPattern } from '../../../generated-types/SurveyPattern'
import { defaultCloverleaf, defaultSweep } from '../../../mission/patterns'

const STEPS: Array<{ id: PlanStep; titleKey: string }> = [
  { id: 'area', titleKey: 'plan.step.area' },
  { id: 'pattern', titleKey: 'plan.step.pattern' },
  { id: 'waypoints', titleKey: 'plan.step.waypoints' },
  { id: 'sync', titleKey: 'plan.step.sync' },
]

const MODES: AltitudeMode[] = ['relative', 'amsl', 'agl']

/** Default clearance above home for a generated pattern. */
const DEFAULT_ALT_AGL_M = 50
const ORIGIN = { latitude_deg: 0, longitude_deg: 0 }

/** Tone of each sync status (text comes from `plan.sync.*`). */
const SYNC_TONE: Record<PlanSyncStatus, string> = {
  empty: 'text-muted',
  unsynced: 'text-muted',
  dirty: 'text-warn',
  mismatch: 'text-error',
  synced: 'text-ok',
}

export default function PlanSteps() {
  const { t } = useTranslation()
  const planStep = useUiStore((s) => s.planStep)
  const setPlanStep = useUiStore((s) => s.setPlanStep)
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const setAltitudeMode = useMissionStore((s) => s.setAltitudeMode)
  const waypoints = useMissionStore((s) => s.waypoints)
  const blocks = useMissionStore((s) => s.blocks)
  const home = useMissionStore((s) => s.home)
  const dirty = useMissionStore((s) => s.dirty)
  const fcMatches = useMissionStore((s) => s.fcMatches)
  const lastSyncedHash = useMissionStore((s) => s.lastSyncedHash)
  const verifying = useMissionStore((s) => s.verifying)

  // Pattern form state, shared by the Area and Pattern steps (the boundary
  // feeds Generate).
  const [kind, setKind] = useState<PatternKind>('sweep')
  const [sweep, setSweep] = useState<SurveyPattern>(() =>
    defaultSweep(ORIGIN, (home?.[2] ?? 0) + DEFAULT_ALT_AGL_M),
  )
  const [clover, setClover] = useState<CloverleafPattern>(() =>
    defaultCloverleaf(ORIGIN, (home?.[2] ?? 0) + DEFAULT_ALT_AGL_M),
  )
  const [size, setSize] = useState({ width: 500, height: 500 })
  // Two-click confirm state lives here so collapsing the Sync card never
  // leaves a stale armed button.
  const [confirmClearPlan, setConfirmClearPlan] = useState(false)
  const [confirmClearFc, setConfirmClearFc] = useState(false)
  // Import notices render in the Waypoints step.
  const [importNotice, setImportNotice] = useState<string[]>([])

  const sync = planSyncStatus({
    itemCount: waypoints.length + blocks.reduce((n, b) => n + b.children.length, 0),
    dirty,
    fcMatches,
    lastSyncedHash,
  })

  return (
    <div className="flex h-full flex-col gap-2">
      {/* Header: always visible. Altitude mode + the one sync badge (P4). */}
      <div className="panel rounded p-2">
        <div className="mb-1.5 text-xs uppercase tracking-wide text-muted">{t('plan.altitude')}</div>
        <div className="flex rounded-md border border-line bg-canvas p-0.5 text-xs">
          {MODES.map((m) => (
            <button
              key={m}
              onClick={() => setAltitudeMode(m)}
              className={`flex-1 rounded px-1 py-1 transition-colors ${
                altitudeMode === m ? 'bg-accent text-canvas' : 'text-muted hover:text-ink'
              }`}
            >
              {t(`plan.mode.${m}`)}
            </button>
          ))}
        </div>
        {sync !== 'empty' && (
          <div className={`mt-1 text-right text-xs ${SYNC_TONE[sync]}`}>
            {verifying ? t('plan.sync.verifying') : t(`plan.sync.${sync}`)}
          </div>
        )}
      </div>

      {STEPS.map(({ id, titleKey }) => (
        <StepCard
          key={id}
          title={t(titleKey)}
          open={planStep === id}
          onToggle={() => setPlanStep(id)}
        >
          {id === 'area' ? <AreaStep size={size} onSize={setSize} /> : null}
          {id === 'pattern' ? (
            <PatternStep
              kind={kind}
              onKind={setKind}
              sweep={sweep}
              onSweep={setSweep}
              clover={clover}
              onClover={setClover}
              size={size}
            />
          ) : null}
          {id === 'waypoints' ? (
            <WaypointsStep importNotice={importNotice} />
          ) : null}
          {id === 'sync' ? (
            <SyncStep
              confirmClearPlan={confirmClearPlan}
              setConfirmClearPlan={setConfirmClearPlan}
              confirmClearFc={confirmClearFc}
              setConfirmClearFc={setConfirmClearFc}
              onImportNotice={setImportNotice}
            />
          ) : null}
        </StepCard>
      ))}
    </div>
  )
}

function StepCard({
  title,
  open,
  onToggle,
  children,
}: {
  title: string
  open: boolean
  onToggle: () => void
  children: React.ReactNode
}) {
  return (
    <div className="panel rounded">
      <button
        onClick={onToggle}
        aria-expanded={open}
        className="flex w-full items-center justify-between px-2 py-2 text-left text-sm"
      >
        <span className="font-medium text-ink">{title}</span>
        <span className={`text-muted transition-transform ${open ? 'rotate-90' : ''}`}>▸</span>
      </button>
      {open && <div className="border-t border-line p-2">{children}</div>}
    </div>
  )
}