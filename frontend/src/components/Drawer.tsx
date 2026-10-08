import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { useLinkStore } from '../stores/link'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore, type DrawerId } from '../stores/ui'
import { planSyncStatus, type PlanSyncStatus } from '../mission/sync'
import { useMissionStore } from '../stores/mission'
import { kindsBySeq } from '../mission/lineKinds'
import MissionProgressCard from './panels/MissionProgressCard'
import { useMissionProgress } from '../hooks/useMissionProgress'
import type { LineKind } from '../generated-types/LineKind'
import { fcStatusLabel } from '../util/linkLabel'

/** Tone of each sync status (mirrors the planning panel). */
const SYNC_TONE: Record<PlanSyncStatus, string> = {
  empty: 'text-muted',
  unsynced: 'text-muted',
  dirty: 'text-warn',
  mismatch: 'text-error',
  synced: 'text-ok',
}

/** Colour of a pattern line kind, matching the map polylines. */
const KIND_TONE: Record<LineKind, string> = {
  survey: 'text-accent',
  tie: 'text-warn',
  calibration: 'text-mag',
}

export default function Drawer() {
  const { t } = useTranslation()
  const drawer = useUiStore((s) => s.drawer)
  const closeDrawer = useUiStore((s) => s.closeDrawer)

  if (!drawer) return null

  return (
    <aside
      className="flex w-[280px] shrink-0 flex-col border-r border-line bg-panel"
      style={{ width: 280 }}
    >
      <div className="flex items-center justify-between border-b border-line px-3 py-2">
        <h2 className="text-sm font-medium">{t(drawerTitle(drawer))}</h2>
        <button
          onClick={closeDrawer}
          className="touch-target rounded px-2 text-sm text-muted hover:text-ink"
        >
          ×
        </button>
      </div>
      <div className="flex-1 overflow-y-auto p-3">
        {drawer === 'missions' && <MissionsSection />}
        {drawer === 'vehicles' && <VehiclesSection />}
        {drawer === 'layers' && <LayersSection />}
      </div>
    </aside>
  )
}

function drawerTitle(id: NonNullable<DrawerId>): string {
  switch (id) {
    case 'missions':
      return 'panels.missions'
    case 'vehicles':
      return 'panels.vehicles'
    case 'layers':
      return 'panels.layers'
  }
}

/**
 * The mission, read two ways: while editing the plan shows what it is built
 * from, while flying it shows how far through it the vehicle is —
 * "Sweep 1 · 250 m" tells an operator on the ground nothing (issues.md #37).
 */
function MissionsSection() {
  const view = useUiStore((s) => s.view)
  return view === 'flight' ? <FlightMissionSection /> : <PlanSection />
}

/** Progress + the waypoint list with the active item, for the flight view. */
function FlightMissionSection() {
  const { t } = useTranslation()
  const { items, progress } = useMissionProgress()
  const lastPattern = useMissionStore((s) => s.lastPattern)
  const focus = useMissionStore((s) => s.focus)
  const kinds = useMemo(() => kindsBySeq(lastPattern?.lines ?? []), [lastPattern])

  if (items.length === 0) return <MissionProgressCard />

  return (
    <div className="space-y-2">
      <MissionProgressCard />
      <h3 className="pt-1 text-xs uppercase tracking-wide text-muted">{t('panels.planItems')}</h3>
      <div className="space-y-1">
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
              <span className="mono shrink-0 text-xs text-muted">
                {item.altitude_m.toFixed(1)} m
              </span>
            </button>
          )
        })}
      </div>
    </div>
  )
}

/** The plan as it is being built: item count, sync state and the pattern lines. */
function PlanSection() {
  const { t } = useTranslation()
  const waypoints = useMissionStore((s) => s.waypoints)
  const blocks = useMissionStore((s) => s.blocks)
  const lastPattern = useMissionStore((s) => s.lastPattern)
  const currentSeq = useMissionStore((s) => s.currentSeq)
  const dirty = useMissionStore((s) => s.dirty)
  const lastSyncedHash = useMissionStore((s) => s.lastSyncedHash)
  const fcMatches = useMissionStore((s) => s.fcMatches)
  const focus = useMissionStore((s) => s.focus)

  const status = planSyncStatus({
    itemCount: waypoints.length + blocks.reduce((n, b) => n + b.children.length, 0),
    dirty,
    fcMatches,
    lastSyncedHash,
  })
  const lines = lastPattern?.lines ?? []

  if (status === 'empty') {
    return <div className="panel rounded p-3 text-sm text-muted">{t('plan.empty')}</div>
  }

  return (
    <div className="space-y-2">
      <div className="panel rounded px-3 py-2 text-sm">
        <div className="flex items-center justify-between">
          <span className="text-muted">{t('plan.items')}</span>
          <span className="mono">{waypoints.length}</span>
        </div>
        <div className="mt-1 flex items-center justify-between">
          <span className="text-muted">{t('panels.status')}</span>
          <span className={`text-xs ${SYNC_TONE[status]}`}>{t(`plan.sync.${status}`)}</span>
        </div>
        {currentSeq !== null && (
          <div className="mt-1 flex items-center justify-between">
            <span className="text-muted">{t('panels.current')}</span>
            <span className="mono">
              {currentSeq + 1} / {waypoints.length}
            </span>
          </div>
        )}
      </div>

      {lines.length > 0 && (
        <>
          <h3 className="pt-1 text-xs uppercase tracking-wide text-muted">
            {t('panels.planItems')}
          </h3>
          <div className="space-y-1">
            {lines.slice(0, 24).map((line) => (
              <button
                key={`${line.kind}-${line.id}`}
                onClick={() => focus(line.start_seq)}
                className="flex w-full touch-target items-center justify-between rounded border border-line bg-canvas px-3 py-2 text-sm hover:bg-panel"
              >
                <span className={KIND_TONE[line.kind]}>
                  {t(`plan.pattern.kind.${kindNameKey(line.kind)}`)} {line.id}
                </span>
                <span className="mono text-xs text-muted">{line.length_m.toFixed(0)} m</span>
              </button>
            ))}
          </div>
        </>
      )}
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

/** Pattern-line kind to the `plan.pattern.kind.*` preset name it came from. */
function kindNameKey(kind: LineKind): string {
  return kind === 'calibration' ? 'cloverleaf' : 'sweep'
}

function VehiclesSection() {
  const { t } = useTranslation()
  const link = useLinkStore((s) => s.link)
  const heartbeat = useTelemetryStore((s) => s.snapshot?.heartbeat ?? null)

  const stateLabel = fcStatusLabel(t, link)
  const tone = link?.fc_alive ? 'text-ok' : 'text-error'

  return (
    <div className="space-y-1">
      <div className="panel flex items-center justify-between rounded px-3 py-2 text-sm">
        <span>{t('panels.vehicle')}</span>
        <span className={`mono ${tone}`}>{stateLabel}</span>
      </div>
      {heartbeat && (
        <div className="panel flex items-center justify-between rounded px-3 py-2 text-sm">
          <span>
            Sys {heartbeat.system_id} / {heartbeat.vehicle_type}
          </span>
          <span className="mono text-muted">{heartbeat.autopilot}</span>
        </div>
      )}
    </div>
  )
}

/**
 * Map layers. Only the two the map actually has are switchable; the terrain
 * layers are listed as "not loaded" rather than pretending to be toggles
 * (issues.md #35 — the old list was four hardcoded ON/OFF labels).
 */
function LayersSection() {
  const { t } = useTranslation()
  const showImagery = useUiStore((s) => s.showImagery)
  const showGrid = useUiStore((s) => s.showGrid)
  const toggleImagery = useUiStore((s) => s.toggleImagery)
  const toggleGrid = useUiStore((s) => s.toggleGrid)

  const live = [
    { key: 'terrain.imagery', on: showImagery, toggle: toggleImagery },
    { key: 'terrain.grid', on: showGrid, toggle: toggleGrid },
  ]

  return (
    <div className="space-y-3">
      <div className="space-y-1">
        {live.map(({ key, on, toggle }) => (
          <button
            key={key}
            onClick={toggle}
            aria-pressed={on}
            className="flex w-full touch-target items-center justify-between rounded border border-line bg-canvas px-3 py-2 text-sm hover:bg-panel"
          >
            <span className="text-ink">{t(key)}</span>
            <span className={`mono text-xs ${on ? 'text-ok' : 'text-muted'}`}>
              {on ? t('layers.on') : t('layers.off')}
            </span>
          </button>
        ))}
      </div>
      <div>
        <div className="mb-1 text-xs uppercase tracking-wide text-muted">
          {t('layers.notLoaded')}
        </div>
        <ul className="space-y-1">
          {['terrain.dtm', 'terrain.dsm'].map((key) => (
            <li
              key={key}
              className="rounded border border-dashed border-line px-3 py-2 text-sm text-muted"
            >
              {t(key)}
              <div className="text-[11px]">{t('layers.terrainLater')}</div>
            </li>
          ))}
        </ul>
      </div>
    </div>
  )
}
