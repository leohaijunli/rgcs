// Step 3 — Waypoints (P2 §4.2): the plan's waypoint list with the
// distance-from-vehicle readout (issues.md #37), coordinate entry, and an
// ItemEditor that expands inline under the selected row (P5: the old editor
// was the last block of a scrolling panel, so selecting changed nothing
// visible).

import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { commandUsesCoordinate, DEFAULT_ALT_AGL_M, MIN_AGL_GROUND_M } from '../../../mission/compile'
import { formatDms, parseCoordinates } from '../../../mission/coords'
import { formatDistance } from '../../../mission/progress'
import { haversineM } from '../../../mission/geo'
import { orderedMissionItems } from '../../../mission/planfile'
import { compileWaypoints } from '../../../mission/compile'
import { useMissionStore } from '../../../stores/mission'
import { useTelemetryStore } from '../../../stores/telemetry'
import { useUiStore } from '../../../stores/ui'
import { useTerrainStore } from '../../../stores/terrain'
import type { PlannedWaypoint } from '../../../generated-types/PlannedWaypoint'

/** Two-line height readout under each waypoint. In terrain mode the AGL is
 * what the operator asked for (the Pattern input), so it leads and the
 * absolute AMSL follows; every other mode keeps AMSL first with the AGL
 * second (operator request: show both). Both lines need a ground reference;
 * in terrain mode a missing one is called out instead of leaving a bare AMSL
 * that looks terrain-following. */
function HeightReadout({
  wp,
  groundM,
  relativeToUnknownHome,
  terrainMode,
  homeAmsl,
}: {
  wp: PlannedWaypoint
  /** Ground (AMSL) under the waypoint, `null` when nothing is loaded. */
  groundM: number | null
  relativeToUnknownHome: boolean
  terrainMode: boolean
  homeAmsl: number | null
}) {
  const { t } = useTranslation()
  const amsl = wp.altitude.meters
  const agl = groundM != null && groundM > MIN_AGL_GROUND_M ? amsl - groundM : null
  if (relativeToUnknownHome) {
    return <span className="mono block text-muted">{amsl.toFixed(1)} m above HOME</span>
  }
  if (terrainMode) {
    return agl != null ? (
      <>
        <span className="mono block text-ink">{agl.toFixed(1)} m AGL</span>
        <span className="mono block text-[10px] text-muted">{amsl.toFixed(1)} m AMSL</span>
      </>
    ) : (
      <>
        <span className="mono block text-muted">{amsl.toFixed(1)} m AMSL</span>
        <span className="mono block text-[10px] text-warn">{t('plan.aglUnknown')}</span>
      </>
    )
  }
  return (
    <>
      <span className="mono block text-muted">{amsl.toFixed(1)} m AMSL</span>
      {agl != null ? (
        <span className="mono block text-[10px] text-muted">{agl.toFixed(1)} m AGL</span>
      ) : homeAmsl != null && homeAmsl !== 0 ? (
        <span className="mono block text-[10px] text-muted">
          {(amsl - homeAmsl).toFixed(1)} m above HOME
        </span>
      ) : null}
    </>
  )
}

export default function WaypointsStep({ importNotice }: { importNotice: string[] }) {
  const { t } = useTranslation()
  const waypoints = useMissionStore((s) => s.waypoints)
  const selectedSeq = useMissionStore((s) => s.selectedSeq)
  const currentSeq = useMissionStore((s) => s.currentSeq)
  const select = useMissionStore((s) => s.select)
  const addWaypoint = useMissionStore((s) => s.addWaypoint)
  const addWaypointAt = useMissionStore((s) => s.addWaypointAt)
  const removeWaypoint = useMissionStore((s) => s.removeWaypoint)
  const removeWaypoints = useMissionStore((s) => s.removeWaypoints)
  const setAltitudeMany = useMissionStore((s) => s.setAltitudeMany)
  const moveWaypoint = useMissionStore((s) => s.moveWaypoint)
  const home = useMissionStore((s) => s.home)
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const blocks = useMissionStore((s) => s.blocks)
  const planBase = useMissionStore((s) => s.planBase)
  const mapCenter = useUiStore((s) => s.mapCenter)
  const groundBySeq = useTerrainStore((s) => s.groundBySeq)
  const pos = useTelemetryStore((s) => s.snapshot?.global_position ?? null)
  const homeAmsl = home?.[2] ?? 0
  // Relative/AGL heights with no HOME elevation: the stored "AMSL" is really
  // height above HOME, so the row must say that instead of AMSL / AGL.
  const relativeToUnknownHome =
    !(home && home[2] !== 0) && (altitudeMode === 'relative' || altitudeMode === 'agl')

  const selected = selectedSeq !== null ? waypoints[selectedSeq] ?? null : null
  const [dragSeq, setDragSeq] = useState<number | null>(null)
  const [coordText, setCoordText] = useState('')
  // U17c: batch selection (row checkboxes) — separate from `selectedSeq`, which
  // stays the single map-linked row. `anchorRef` is the last checkbox clicked,
  // for Shift-range selection.
  const [checked, setChecked] = useState<number[]>([])
  const anchorRef = useRef<number | null>(null)
  const [batchAlt, setBatchAlt] = useState('')

  // Keep the batch selection valid when rows are added or removed.
  useEffect(() => {
    setChecked((prev) => prev.filter((i) => i < waypoints.length))
  }, [waypoints.length])

  const toggleChecked = (idx: number, shift: boolean) => {
    // Capture the anchor BEFORE the store update: `anchorRef.current = idx`
    // below runs synchronously, while the `setChecked` updater runs later, so
    // reading the ref inside the updater would see the new index and collapse
    // the Shift range to a single row.
    const anchor = anchorRef.current
    setChecked((prev) => {
      if (shift && anchor !== null) {
        const lo = Math.min(anchor, idx)
        const hi = Math.max(anchor, idx)
        const next = new Set(prev)
        for (let i = lo; i <= hi; i += 1) next.add(i)
        return [...next].sort((a, b) => a - b)
      }
      return prev.includes(idx)
        ? prev.filter((i) => i !== idx)
        : [...prev, idx].sort((a, b) => a - b)
    })
    anchorRef.current = idx
  }

  const allChecked = waypoints.length > 0 && checked.length === waypoints.length

  // Batch delete is one undoable edit (store `removeWaypoints`), so Ctrl+Z
  // restores the whole selection — that is the explicit undo boundary.
  const deleteChecked = () => {
    removeWaypoints(checked)
    setChecked([])
    anchorRef.current = null
  }

  const applyCheckedAltitude = () => {
    const amslM = parseFloat(batchAlt)
    if (!Number.isFinite(amslM)) return
    setAltitudeMany(checked, amslM)
    setBatchAlt('')
  }

  const flyableCount =
    orderedMissionItems(
      compileWaypoints(waypoints, altitudeMode, homeAmsl),
      blocks,
      planBase ?? undefined,
    ).length

  const onDrop = (targetSeq: number) => {
    if (dragSeq === null || dragSeq === targetSeq) return
    moveWaypoint(dragSeq, targetSeq)
    setDragSeq(null)
  }

  // Default a new waypoint to the selected position (inheriting its altitude)
  // else the map centre, at the default clearance above home (issues.md #14).
  const addAt = () => {
    const latDeg = selected ? selected.position.latitude_deg : mapCenter?.lat
    const lonDeg = selected ? selected.position.longitude_deg : mapCenter?.lon
    if (latDeg == null || lonDeg == null) return
    addWaypoint(latDeg, lonDeg, selected ? selected.altitude.meters : homeAmsl + DEFAULT_ALT_AGL_M)
  }

  // Coordinate entry (WS-G G2): a handheld GPS or an RTK base report is typed
  // or pasted here, so a position does not have to be eyeballed on the map.
  const coords = coordText.trim() === '' ? null : parseCoordinates(coordText)
  const submitCoords = () => {
    if (!coords?.ok) return
    addWaypointAt(coords.value.lat, coords.value.lon)
    setCoordText('')
  }

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between px-1">
        <span className="text-xs uppercase tracking-wide text-muted">
          {t('plan.items')} · {flyableCount}
        </span>
        {waypoints.length > 0 && (
          <button
            onClick={() => setChecked(allChecked ? [] : waypoints.map((_, i) => i))}
            className="text-[11px] text-muted hover:text-ink"
          >
            {allChecked ? t('plan.batch.clear') : t('plan.batch.all')}
          </button>
        )}
      </div>

      {/* U17c: batch actions for the checked rows. Delete and the altitude set
       * are each a single undoable edit (planned undo boundary). */}
      {checked.length > 0 && (
        <div className="space-y-1.5 rounded border border-line bg-canvas p-1.5">
          <div className="flex items-center justify-between text-[11px]">
            <span className="text-muted">{t('plan.batch.selected', { n: checked.length })}</span>
            <button onClick={() => setChecked([])} className="text-muted hover:text-ink">
              {t('plan.batch.clear')}
            </button>
          </div>
          <div className="flex items-center gap-1">
            <input
              type="number"
              step="0.5"
              value={batchAlt}
              onChange={(e) => setBatchAlt(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && applyCheckedAltitude()}
              placeholder={t('plan.batch.altPlaceholder')}
              aria-label={t('plan.batch.altPlaceholder')}
              className="mono min-w-0 flex-1 rounded border border-line bg-panel px-1.5 py-1 text-xs text-ink"
            />
            <button
              onClick={applyCheckedAltitude}
              disabled={!Number.isFinite(parseFloat(batchAlt))}
              className="rounded border border-line bg-panel px-2 py-1 text-xs text-ink hover:bg-canvas disabled:opacity-40"
            >
              {t('plan.batch.setAlt')}
            </button>
          </div>
          <button
            onClick={deleteChecked}
            className="w-full rounded border border-error/50 bg-error/10 px-2 py-1 text-xs text-error hover:bg-error/20"
          >
            {t('plan.batch.delete', { n: checked.length })}
          </button>
        </div>
      )}

      {waypoints.length === 0 && blocks.length === 0 ? (
        <div className="rounded border border-line bg-canvas p-2 text-xs text-muted">{t('plan.empty')}</div>
      ) : (
        <ul className="space-y-1">
          {waypoints.map((wp, idx) => {
          // Flight-state colouring (operator request): flown = dimmed row with
          // a green index, the FC's target = accent, future = plain.
          const flown = currentSeq != null && idx < currentSeq
          const isActive = idx === currentSeq
          return (
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
                    : flown
                      ? 'border-line bg-panel opacity-55'
                      : 'border-line bg-panel hover:bg-canvas'
                }`}
              >
                <input
                  type="checkbox"
                  checked={checked.includes(idx)}
                  draggable={false}
                  onClick={(e) => e.stopPropagation()}
                  onMouseDown={(e) => e.stopPropagation()}
                  onChange={(e) => toggleChecked(idx, (e.nativeEvent as MouseEvent).shiftKey)}
                  aria-label={t('plan.batch.selectRow', { seq: idx })}
                  className="h-3.5 w-3.5 shrink-0"
                />
                <span
                  className={`mono w-6 shrink-0 text-right ${isActive ? 'text-accent' : flown ? 'text-ok' : 'text-muted'}`}
                >
                  {idx}
                </span>
                {commandUsesCoordinate(wp.command) ? (
                  <>
                    <span className="mono min-w-0 flex-1 truncate" title={`${wp.position.latitude_deg.toFixed(6)}, ${wp.position.longitude_deg.toFixed(6)}`}>
                      {wp.position.latitude_deg.toFixed(6)}
                      <span className="text-muted"> · </span>
                      {wp.position.longitude_deg.toFixed(6)}
                    </span>
                    <span className="min-w-0 max-w-[50%] shrink text-right">
                      <HeightReadout
                        wp={wp}
                        groundM={groundBySeq?.[idx] ?? null}
                        relativeToUnknownHome={relativeToUnknownHome}
                        terrainMode={altitudeMode === 'terrain'}
                        homeAmsl={home?.[2] ?? null}
                      />
                      {pos && (
                        <span className="mono block text-[10px] text-muted">
                          {formatDistance(
                            haversineM(
                              { latitude_deg: pos.latitude_deg, longitude_deg: pos.longitude_deg },
                              wp.position,
                            ),
                          )}
                        </span>
                      )}
                    </span>
                  </>
                ) : (
                  <span className="mono min-w-0 flex-1 truncate text-muted">
                    {t('plan.commandOnly', { command: wp.command })}
                  </span>
                )}
                {idx === currentSeq ? (
                  <span data-testid="mission-current-marker" className="shrink-0 rounded bg-ok/20 px-1 text-[10px] text-ok">
                    {t('plan.current')}
                  </span>
                ) : null}
              </div>
              {/* Inline editor: under the row it belongs to (P5/A8). */}
              {idx === selectedSeq && selected ? <ItemEditor seq={idx} waypoint={wp} /> : null}
            </li>
          )
        })}
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

      {importNotice.length > 0 && (
        <div className="rounded border border-warn px-2 py-1 text-xs text-warn">
          {t('plan.importUnsupported', { items: importNotice.join(', ') })}
        </div>
      )}

      {/* Coordinate entry + add/remove. */}
      <div className="flex items-center gap-1.5">
        <input
          value={coordText}
          onChange={(e) => setCoordText(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && submitCoords()}
          placeholder={t('plan.coord.placeholder')}
          aria-label={t('plan.coord.title')}
          className="mono touch-target min-w-0 flex-1 rounded border border-line bg-canvas px-1.5 py-1 text-sm text-ink"
        />
        <button
          disabled={!coords?.ok}
          onClick={submitCoords}
          className="touch-target rounded-md border border-line bg-panel px-2 text-sm hover:bg-canvas disabled:opacity-40"
        >
          {t('plan.coord.add')}
        </button>
      </div>
      {coords && !coords.ok ? (
        <div className="text-[11px] text-error">{t(`plan.coord.error.${coords.error}`)}</div>
      ) : coords?.ok ? (
        <div className="mono truncate text-[11px] text-muted">
          {formatDms(coords.value.lat, 'lat')} · {formatDms(coords.value.lon, 'lon')}
        </div>
      ) : null}

      <div className="grid grid-cols-2 gap-1.5">
        <button
          onClick={addAt}
          disabled={selected == null && mapCenter == null}
          className="rounded-md border border-line bg-panel px-2 py-1.5 text-sm hover:bg-canvas disabled:opacity-40"
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
    </div>
  )
}

/** Inline editor's confirm-timer cleanup lives in the editor itself. */
function ItemEditor({ seq, waypoint }: { seq: number; waypoint: PlannedWaypoint }) {
  const { t } = useTranslation()
  const updatePosition = useMissionStore((s) => s.updatePosition)
  const updateAltitude = useMissionStore((s) => s.updateAltitude)
  const toggleWaypointLock = useMissionStore((s) => s.toggleWaypointLock)
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const setCurrent = useMissionStore((s) => s.setCurrent)
  const coordinate = commandUsesCoordinate(waypoint.command)
  const confirmTimer = useRef<number | undefined>(undefined)
  useEffect(() => () => window.clearTimeout(confirmTimer.current), [])

  return (
    <div className="mt-1 rounded border border-accent/40 bg-canvas p-2 text-sm">
      <div className="mb-1.5 text-xs uppercase tracking-wide text-muted">
        {t('plan.properties')} · WP {seq}
      </div>
      <div className="space-y-1.5">
        {!coordinate ? (
          <div className="text-xs text-muted">
            {t('plan.commandOnly', { command: waypoint.command })}
          </div>
        ) : null}
        <label className={`flex items-center gap-2 ${coordinate ? '' : 'hidden'}`}>
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
        <label className={`flex items-center gap-2 ${coordinate ? '' : 'hidden'}`}>
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
        <label className={`flex items-center gap-2 ${coordinate ? '' : 'hidden'}`}>
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
        {/* Height pin (plan review): a locked waypoint keeps its altitude
         * through terrain-mode re-drapes — the operator's manual override. */}
        {coordinate && altitudeMode === 'terrain' && (
          <label className="flex items-center gap-2 text-xs">
            <input
              type="checkbox"
              checked={waypoint.locked === true}
              onChange={() => toggleWaypointLock(seq)}
              className="h-3.5 w-3.5"
            />
            <span className="text-muted">{t('plan.lockHeight')}</span>
          </label>
        )}
        <div className="flex items-center justify-between">
          <span className="text-muted">{t('plan.command')}</span>
          <span className="mono">{waypoint.command === 16 ? 'NAV_WAYPOINT' : waypoint.command}</span>
        </div>
        <button
          onClick={() => void setCurrent(seq)}
          className="w-full rounded border border-line bg-panel px-2 py-1 text-xs hover:bg-canvas"
        >
          {t('plan.goTo')}
        </button>
      </div>
    </div>
  )
}
