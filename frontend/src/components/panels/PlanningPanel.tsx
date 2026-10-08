import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { compileWaypoints, DEFAULT_ALT_AGL_M } from '../../mission/compile'
import { exportPlanFile, importPlanFile, orderedMissionItems } from '../../mission/planfile'
import { planSyncStatus, type PlanSyncStatus } from '../../mission/sync'
import PatternPanel from './PatternPanel'
import { useLinkStore } from '../../stores/link'
import { useUiStore } from '../../stores/ui'
import { useMissionStore, type AltitudeMode } from '../../stores/mission'
import type { PlannedWaypoint } from '../../generated-types/PlannedWaypoint'

const MODES: AltitudeMode[] = ['relative', 'amsl', 'agl']

/** How long a destructive button stays armed while waiting for the second click. */
const CONFIRM_WINDOW_MS = 5000

/** Tone of each sync status (the text comes from `plan.sync.*`). */
const SYNC_TONE: Record<PlanSyncStatus, string> = {
  empty: 'text-muted',
  unsynced: 'text-muted',
  dirty: 'text-warn',
  mismatch: 'text-error',
  synced: 'text-ok',
}

export default function PlanningPanel() {
  const { t } = useTranslation()
  const link = useLinkStore((s) => s.link)
  const connected = Boolean(link?.fc_alive)
  const waypoints = useMissionStore((s) => s.waypoints)
  const selectedSeq = useMissionStore((s) => s.selectedSeq)
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const busy = useMissionStore((s) => s.busy)
  const syncState = useMissionStore((s) => s.syncState)
  const lastEvent = useMissionStore((s) => s.lastEvent)
  const dirty = useMissionStore((s) => s.dirty)
  const fcMatches = useMissionStore((s) => s.fcMatches)
  const planBase = useMissionStore((s) => s.planBase)
  const home = useMissionStore((s) => s.home)
  const blocks = useMissionStore((s) => s.blocks)
  const lastSyncedHash = useMissionStore((s) => s.lastSyncedHash)
  const verifying = useMissionStore((s) => s.verifying)
  const select = useMissionStore((s) => s.select)
  const setAltitudeMode = useMissionStore((s) => s.setAltitudeMode)
  const addWaypoint = useMissionStore((s) => s.addWaypoint)
  const removeWaypoint = useMissionStore((s) => s.removeWaypoint)
  const moveWaypoint = useMissionStore((s) => s.moveWaypoint)
  const upload = useMissionStore((s) => s.upload)
  const download = useMissionStore((s) => s.download)
  const clear = useMissionStore((s) => s.clear)
  const clearPlan = useMissionStore((s) => s.clearPlan)

  const selected = selectedSeq !== null ? waypoints[selectedSeq] ?? null : null
  const [dragSeq, setDragSeq] = useState<number | null>(null)
  const [importNotice, setImportNotice] = useState<string[]>([])
  const [confirmClearPlan, setConfirmClearPlan] = useState(false)
  const confirmTimer = useRef<number | undefined>(undefined)
  useEffect(() => () => window.clearTimeout(confirmTimer.current), [])
  const mapCenter = useUiStore((s) => s.mapCenter)
  const homeAmsl = home?.[2] ?? 0

  // Compiled items + complex-item children, in the order they will be flown.
  const flyable = useMemo(
    () =>
      orderedMissionItems(
        compileWaypoints(waypoints, altitudeMode, homeAmsl),
        blocks,
        planBase ?? undefined,
      ),
    [waypoints, altitudeMode, homeAmsl, blocks, planBase],
  )

  const onDrop = (targetSeq: number) => {
    if (dragSeq === null || dragSeq === targetSeq) return
    moveWaypoint(dragSeq, targetSeq)
    setDragSeq(null)
  }

  const onImport = async () => {
    const result = await importPlanFile()
    if (!result) return
    useMissionStore.getState().applyImport(result)
    setImportNotice(result.unsupported)
  }

  const onExport = async () => {
    // Export needs items already compiled to the selected frame.
    await exportPlanFile(compileWaypoints(waypoints, altitudeMode, homeAmsl), altitudeMode, {
      base: planBase ?? undefined,
      home,
      blocks,
    })
  }

  // Default a new waypoint to the selected position (inheriting its altitude)
  // else the map centre, at the default clearance above home (issues.md #14).
  const addAt = () => {
    const latDeg = selected ? selected.position.latitude_deg : mapCenter?.lat
    const lonDeg = selected ? selected.position.longitude_deg : mapCenter?.lon
    if (latDeg == null || lonDeg == null) return
    addWaypoint(latDeg, lonDeg, selected ? selected.altitude.meters : homeAmsl + DEFAULT_ALT_AGL_M)
  }

  // Clear the *local* plan (the FC keeps its mission); two clicks, as for RTL.
  const pressClearPlan = () => {
    if (!confirmClearPlan) {
      setConfirmClearPlan(true)
      window.clearTimeout(confirmTimer.current)
      confirmTimer.current = window.setTimeout(() => setConfirmClearPlan(false), CONFIRM_WINDOW_MS)
      return
    }
    window.clearTimeout(confirmTimer.current)
    setConfirmClearPlan(false)
    clearPlan()
  }

  const sync = planSyncStatus({
    itemCount: flyable.length,
    dirty,
    fcMatches,
    lastSyncedHash,
  })

  return (
    <div className="flex h-full flex-col gap-2">
      <div className="panel rounded p-2">
        <div className="mb-1.5 text-xs uppercase tracking-wide text-muted">{t('plan.altitude')}</div>
        <div className="flex rounded-md border border-line bg-canvas p-0.5 text-xs">
          {MODES.map((m) => (
            <button
              key={m}
              onClick={() => setAltitudeMode(m)}
              className={`flex-1 rounded px-1 py-1 transition-colors ${
                altitudeMode === m
                  ? 'bg-accent text-canvas'
                  : 'text-muted hover:text-ink'
              }`}
            >
              {t(`plan.mode.${m}`)}
            </button>
          ))}
        </div>
      </div>

      <PatternPanel />

      <div className="min-h-0 flex-1 overflow-auto">
        <div className="mb-1.5 flex items-center justify-between px-1">
          <span className="text-xs uppercase tracking-wide text-muted">
            {t('plan.items')} · {flyable.length}
          </span>
        </div>
        {flyable.length === 0 ? (
          <div className="panel rounded p-3 text-xs text-muted">{t('plan.empty')}</div>
        ) : (
          <ul className="space-y-1">
            {waypoints.map((wp, idx) => (
              <li key={idx}>
                <div
                  draggable
                  onDragStart={() => setDragSeq(idx)}
                  onDragOver={(e) => e.preventDefault()}
                  onDrop={() => onDrop(idx)}
                  onClick={() => select(idx)}
                  className={`flex cursor-pointer items-center gap-2 rounded border px-2 py-1.5 text-sm transition-colors ${
                    idx === selectedSeq
                      ? 'border-accent bg-accent/10'
                      : 'border-line bg-panel hover:bg-canvas'
                  }`}
                >
                  <span className="mono w-6 shrink-0 text-right text-muted">{idx}</span>
                  <span className="mono min-w-0 flex-1 truncate">
                    {t('plan.lat')} {wp.position.latitude_deg.toFixed(6)}
                    <span className="text-muted"> · </span>
                    {t('plan.lon')} {wp.position.longitude_deg.toFixed(6)}
                  </span>
                  <span className="mono shrink-0 text-muted">
                    {wp.altitude.meters.toFixed(1)} m
                  </span>
                  {idx === 0 ? (
                    <span className="shrink-0 rounded bg-ok/20 px-1 text-[10px] text-ok">
                      {t('plan.current')}
                    </span>
                  ) : null}
                </div>
              </li>
            ))}
          </ul>
        )}
        {blocks.length > 0 && (
          <ul className="mt-1 space-y-1">
            {blocks.map((b) => (
              <li key={`block-${b.index}`}>
                <div className="flex items-center gap-2 rounded border border-dashed border-line bg-canvas/60 px-2 py-1.5 text-sm text-muted">
                  <span className="shrink-0 rounded bg-canvas px-1 text-[10px] uppercase tracking-wide">
                    {t('plan.complex.readOnly')}
                  </span>
                  <span className="min-w-0 flex-1 truncate">
                    {t('plan.complex.label', { type: b.type, count: b.children.length })}
                  </span>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>

      {selected && selectedSeq !== null ? (
        <ItemEditor seq={selectedSeq} waypoint={selected} />
      ) : null}

      {importNotice.length > 0 && (
        <div className="rounded border border-warn px-2 py-1 text-xs text-warn">
          {t('plan.importUnsupported', { items: importNotice.join(', ') })}
        </div>
      )}
      {sync !== 'empty' && (
        <div className={`text-xs ${SYNC_TONE[sync]}`}>
          {verifying ? t('plan.sync.verifying') : t(`plan.sync.${sync}`)}
        </div>
      )}

      <div className="grid grid-cols-2 gap-1.5">
        <button
          onClick={addAt}
          disabled={selected == null && mapCenter == null}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas"
        >
          {t('plan.add')}
        </button>
        <button
          disabled={selectedSeq == null}
          onClick={() => selectedSeq != null && removeWaypoint(selectedSeq)}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
        >
          {t('plan.remove')}
        </button>
      </div>

      <div className="grid grid-cols-3 gap-1.5">
        <button
          disabled={!connected || busy || flyable.length === 0}
          title={!connected ? t('plan.needLink') : undefined}
          onClick={() => void upload()}
          className="rounded-md bg-accent px-2 py-1.5 text-sm text-canvas disabled:opacity-40"
        >
          {t('plan.upload')}
        </button>
        <button
          disabled={!connected || busy}
          onClick={() => void download()}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
        >
          {t('plan.download')}
        </button>
        <button
          disabled={!connected || busy}
          title={!connected ? t('plan.needLink') : undefined}
          onClick={() => void clear()}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
        >
          {t('plan.clear')}
        </button>
      </div>

      <div className="grid grid-cols-3 gap-1.5">
        <button
          onClick={() => void onImport()}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas"
        >
          {t('plan.import')}
        </button>
        <button
          disabled={flyable.length === 0}
          onClick={() => void onExport()}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
        >
          {t('plan.export')}
        </button>
        <button
          disabled={busy || sync === 'empty'}
          onClick={pressClearPlan}
          className={`rounded-md border border-line px-2 py-1.5 text-sm transition-colors disabled:opacity-40 ${
            confirmClearPlan ? 'bg-warn text-canvas' : 'bg-panel hover:bg-canvas'
          }`}
        >
          {confirmClearPlan ? t('plan.confirmClearPlan') : t('plan.clearPlan')}
        </button>
      </div>

      {(busy || lastEvent) && (
        <div className="text-xs text-muted">
          {busy ? (
            <span className="text-accent">{t(`plan.status.${syncState}`)}</span>
          ) : lastEvent && lastEvent.kind === 'failed' ? (
            <span className="text-error">
              {t('plan.failed')}: {lastEvent.message ?? '—'}
            </span>
          ) : lastEvent && lastEvent.kind === 'progress' ? (
            <span>
              {t(`plan.op.${lastEvent.op}`)} {lastEvent.sent}/{lastEvent.total}
            </span>
          ) : (
            lastEvent && <span className="text-ok">{t(`plan.done.${lastEvent.op}`)}</span>
          )}
        </div>
      )}
    </div>
  )
}

function ItemEditor({ seq, waypoint }: { seq: number; waypoint: PlannedWaypoint }) {
  const { t } = useTranslation()
  const updatePosition = useMissionStore((s) => s.updatePosition)
  const updateAltitude = useMissionStore((s) => s.updateAltitude)
  const setCurrent = useMissionStore((s) => s.setCurrent)

  return (
    <div className="panel rounded p-2 text-sm">
      <div className="mb-1.5 text-xs uppercase tracking-wide text-muted">
        {t('plan.properties')} · WP {seq}
      </div>
      <div className="space-y-1.5">
        <label className="flex items-center gap-2">
          <span className="w-10 shrink-0 text-muted">Lat</span>
          <input
            type="number"
            step="0.0000001"
            value={waypoint.position.latitude_deg.toFixed(7)}
            onChange={(e) =>
              updatePosition(seq, parseFloat(e.target.value), waypoint.position.longitude_deg)
            }
            className="mono w-full rounded border border-line bg-canvas px-1.5 py-1"
          />
        </label>
        <label className="flex items-center gap-2">
          <span className="w-10 shrink-0 text-muted">Lon</span>
          <input
            type="number"
            step="0.0000001"
            value={waypoint.position.longitude_deg.toFixed(7)}
            onChange={(e) =>
              updatePosition(seq, waypoint.position.latitude_deg, parseFloat(e.target.value))
            }
            className="mono w-full rounded border border-line bg-canvas px-1.5 py-1"
          />
        </label>
        <label className="flex items-center gap-2">
          <span className="w-10 shrink-0 text-muted">Alt</span>
          <input
            type="number"
            step="0.5"
            value={waypoint.altitude.meters}
            onChange={(e) => updateAltitude(seq, parseFloat(e.target.value))}
            className="mono w-full rounded border border-line bg-canvas px-1.5 py-1"
          />
          <span className="shrink-0 text-[10px] text-muted">AMSL</span>
        </label>
        <div className="flex items-center justify-between">
          <span className="text-muted">{t('plan.command')}</span>
          <span className="mono">{waypoint.command === 16 ? 'NAV_WAYPOINT' : waypoint.command}</span>
        </div>
        {seq === 0 ? (
          <button
            onClick={() => void setCurrent(seq)}
            className="w-full rounded border border-line bg-panel px-2 py-1 text-xs hover:bg-canvas"
          >
            {t('plan.goTo')}
          </button>
        ) : null}
      </div>
    </div>
  )
}
