import { useEffect, useMemo, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { useCesiumViewer } from '../hooks/useCesiumViewer'
import { compileWaypoints, toDisplayItems } from '../mission/compile'
import { kindsBySeq } from '../mission/lineKinds'
import { orderedMissionItems } from '../mission/planfile'
import { useMissionStore } from '../stores/mission'
import { usePolygonStore } from '../stores/polygon'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore } from '../stores/ui'
import MapToolbar from './MapToolbar'

/** Composition shell: viewer lifecycle + store wiring + chrome (issues.md #29). */
export default function MapView() {
  const { t } = useTranslation()
  const containerRef = useRef<HTMLDivElement>(null)
  const { initError, setSnapshot, setMission, setLayers, goHome, lookNorth } =
    useCesiumViewer(containerRef)
  const mapTool = useUiStore((s) => s.mapTool)
  const view = useUiStore((s) => s.view)
  const polygonClosed = usePolygonStore((s) => s.closed)

  // Telemetry updates: drone position, trail, camera follow.
  const snapshot = useTelemetryStore((s) => s.snapshot)
  useEffect(() => {
    setSnapshot(snapshot)
  }, [snapshot, setSnapshot])

  // Mission waypoints: render a polyline + numbered points, re-run when the
  // plan changes. Planned waypoints are compiled to the current frame, and
  // complex-item children are appended in flyable order so an imported survey
  // is drawn (findings 3, ADR-013).
  const waypoints = useMissionStore((s) => s.waypoints)
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const home = useMissionStore((s) => s.home)
  const blocks = useMissionStore((s) => s.blocks)
  const planBase = useMissionStore((s) => s.planBase)
  const selectedSeq = useMissionStore((s) => s.selectedSeq)
  const currentSeq = useMissionStore((s) => s.currentSeq)
  const lastPattern = useMissionStore((s) => s.lastPattern)
  const showHeights = useUiStore((s) => s.showHeights)
  const missionItems = useMemo(
    () =>
      // Drawn in AMSL: in the relative/terrain modes the compiled `z` is only
      // an offset from HOME, which would place the path at the wrong height.
      toDisplayItems(
        orderedMissionItems(
          compileWaypoints(waypoints, altitudeMode, home?.[2] ?? 0),
          blocks,
          planBase ?? undefined,
        ),
        home?.[2] ?? 0,
      ),
    [waypoints, altitudeMode, home, blocks, planBase],
  )
  const showImagery = useUiStore((s) => s.showImagery)
  const showGrid = useUiStore((s) => s.showGrid)
  useEffect(() => {
    setLayers(showImagery, showGrid)
  }, [showImagery, showGrid, setLayers])

  const kinds = useMemo(() => kindsBySeq(lastPattern?.lines ?? []), [lastPattern])
  useEffect(() => {
    setMission(missionItems, selectedSeq, {
      kinds,
      heights: showHeights,
      // Ground plane for the sticks and the AGL chips: HOME's AMSL until a
      // DEM exists (WS-D) — the same plane the vehicle drop line and the AGL
      // compile mode stand on (0 until the plan carries a home position).
      groundM: home?.[2] ?? 0,
      // Flight-state colouring: flown waypoints dim, the target gets a halo.
      currentSeq,
    })
  }, [missionItems, selectedSeq, kinds, showHeights, home, currentSeq, setMission])

  // Tool shortcuts (P3 §4.3): W = add waypoint, P = draw polygon. Planning
  // only; Esc already returns to select (cesium/waypoints.ts).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (view !== 'planning') return
      const tag = (e.target as HTMLElement | null)?.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return
      if (e.metaKey || e.ctrlKey || e.altKey) return
      if (e.key === 'w' || e.key === 'W') {
        useUiStore.getState().setMapTool(useUiStore.getState().mapTool === 'add' ? 'select' : 'add')
      } else if (e.key === 'p' || e.key === 'P') {
        useUiStore.getState().setMapTool(
          useUiStore.getState().mapTool === 'polygon' ? 'select' : 'polygon',
        )
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [view])

  if (initError) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-2 p-6 text-center">
        <div className="text-sm font-medium text-error">Map failed to initialize</div>
        <pre className="mono max-w-full overflow-auto rounded border border-line bg-canvas p-3 text-xs text-ink">
          {initError}
        </pre>
      </div>
    )
  }

  return (
    <div className="relative h-full w-full">
      <div ref={containerRef} className="h-full w-full" />
      <MapToolbar onGoHome={goHome} onNorth={lookNorth} />
      {view === 'planning' && mapTool === 'add' && (
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded border border-line bg-panel/85 px-3 py-1 text-xs text-ink shadow-lg">
          {t('map.addHint')}
        </div>
      )}
      {view === 'planning' && mapTool === 'polygon' && !polygonClosed && (
        // Transient drawing hint (top-centre slot, §4.3); the boundary
        // readout lives in the plan panel's Area step (P2).
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded border border-line bg-panel/85 px-3 py-1 text-xs text-ink shadow-lg">
          {t('map.polygonHint')}
        </div>
      )}
      <CoordinateReadout />
    </div>
  )
}

/** Vehicle position readout, bottom-right corner (§4.3: moved out of the
 * toolbar so the strip carries only tools). */
function CoordinateReadout() {
  const pos = useTelemetryStore((s) => s.snapshot?.global_position ?? null)
  if (!pos) return null
  return (
    <div className="mono pointer-events-none absolute bottom-3 right-3 rounded border border-line bg-panel/85 px-2 py-1 text-right text-[10px] leading-tight text-muted shadow-lg">
      {pos.latitude_deg.toFixed(5)} · {pos.longitude_deg.toFixed(5)}
      <br />
      {Math.round(pos.altitude.meters)} m
    </div>
  )
}
