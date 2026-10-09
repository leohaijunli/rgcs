import { useEffect, useMemo, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { useCesiumViewer } from '../hooks/useCesiumViewer'
import { compileWaypoints, toDisplayItems } from '../mission/compile'
import { kindsBySeq } from '../mission/lineKinds'
import { orderedMissionItems } from '../mission/planfile'
import { polygonArea, polygonPerimeter, selfIntersects } from '../mission/polygon'
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
  const polygon = usePolygonStore((s) => s.vertices)
  const polygonClosed = usePolygonStore((s) => s.closed)
  const polygonCount = polygon.length
  const polygonBad = polygonClosed && selfIntersects(polygon)
  const areaHectares = polygonArea(polygon) / 10000
  const perimeter = polygonPerimeter(polygon)

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
      // Stand the height sticks on the ground: HOME's AMSL altitude until a
      // DEM exists (WS-D), same assumption the AGL mode makes.
      groundM: home?.[2] ?? null,
    })
  }, [missionItems, selectedSeq, kinds, showHeights, home, setMission])

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
      <MapToolbar onGoHome={goHome} onToggleMeasure={() => undefined} onNorth={lookNorth} />
      {view === 'planning' && mapTool === 'add' && (
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded border border-line bg-panel/85 px-3 py-1 text-xs text-ink shadow-lg">
          {t('map.addHint')}
        </div>
      )}
      {view === 'planning' && polygonCount > 0 && (
        <div
          className={`pointer-events-none absolute bottom-3 left-1/2 flex -translate-x-1/2 flex-col items-center gap-0.5 rounded border px-3 py-1 text-xs shadow-lg ${
            polygonBad ? 'border-error/60 bg-error/10 text-error' : 'border-line bg-panel/85 text-ink'
          }`}
        >
          {mapTool === 'polygon' && !polygonClosed && (
            <span className="text-muted">{t('map.polygonHint')}</span>
          )}
          {polygonClosed && <span>{t('map.polygonClosed')}</span>}
          <span className="mono">
            {t('map.polygonReadout', {
              count: polygonCount,
              area: areaHectares.toFixed(1),
              perimeter: Math.round(perimeter),
            })}
          </span>
          {polygonBad && <span className="font-medium">{t('map.polygonSelfIntersect')}</span>}
          <button
            onClick={() => usePolygonStore.getState().reset()}
            className="pointer-events-auto rounded border border-line bg-canvas px-2 py-0.5 text-[11px] text-ink hover:bg-panel"
          >
            {t('map.polygonClear')}
          </button>
        </div>
      )}
    </div>
  )
}
