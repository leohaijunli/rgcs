import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useCesiumViewer } from '../hooks/useCesiumViewer'
import { compileWaypoints, toDisplayItems } from '../mission/compile'
import { kindsBySeq } from '../mission/lineKinds'
import { orderedMissionItems } from '../mission/planfile'
import { useMissionStore } from '../stores/mission'
import { usePolygonStore } from '../stores/polygon'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore } from '../stores/ui'
import { useTerrainStore } from '../stores/terrain'
import MapToolbar from './MapToolbar'

/** Composition shell: viewer lifecycle + store wiring + chrome (issues.md #29). */
export default function MapView() {
  const { t } = useTranslation()
  const containerRef = useRef<HTMLDivElement>(null)
  const { initError, setSnapshot, setMission, setTerrainOverlay, setTerrain3d, setLayers, setImageryStyle, goHome, lookNorth } =
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
  const imageryStyle = useUiStore((s) => s.imageryStyle)
  const showGrid = useUiStore((s) => s.showGrid)
  useEffect(() => {
    setLayers(showGrid)
  }, [showGrid, setLayers])
  useEffect(() => {
    setImageryStyle(imageryStyle)
  }, [imageryStyle, setImageryStyle])

  // Terrain hillshade overlay (plan A): react to the store's overlay.
  const terrainOverlay = useTerrainStore((st) => st.overlay)
  useEffect(() => {
    setTerrainOverlay(terrainOverlay)
  }, [terrainOverlay, setTerrainOverlay])

  // 3D terrain tileset (plan B): react to the store's tileset base URL. A
  // provider load that fails (server gone, bad manifest) rolls the toggle
  // back through fail3d; a stale rejection after a newer switch is dropped.
  const terrain3d = useTerrainStore((st) => st.terrain3d)
  useEffect(() => {
    let cancelled = false
    void setTerrain3d(terrain3d?.url ?? null).catch((e) => {
      if (!cancelled) void useTerrainStore.getState().fail3d(String(e))
    })
    return () => {
      cancelled = true
    }
  }, [terrain3d, setTerrain3d])

  // A tileset server started before a webview reload keeps running —
  // re-attach on boot (terrain3d_status).
  useEffect(() => {
    void useTerrainStore.getState().restore3d()
  }, [])

  // Accurate AGL markers: sample the loaded DEM under every waypoint (the
  // flat HOME plane stays the fallback). The result lives in the terrain
  // store so the waypoint list shows the same ground; debounced for drags.
  const terrainTile = useTerrainStore((st) => st.tile)
  const groundBySeq = useTerrainStore((st) => st.groundBySeq)
  const sampleGround = useTerrainStore((st) => st.sampleGround)
  useEffect(() => {
    if (!terrainTile || missionItems.length === 0) {
      useTerrainStore.setState({ groundBySeq: null })
      return
    }
    const id = setTimeout(() => {
      // MissionItem carries ×1e7 int lat/lon; sample in degrees. Command
      // items (DO_*) have no coordinate and are skipped.
      const points = missionItems
        .filter((it) => it.x !== 0 || it.y !== 0)
        .map((it) => [it.seq, it.y / 1e7, it.x / 1e7] as [number, number, number])
      void sampleGround(points)
    }, 300)
    return () => clearTimeout(id)
  }, [missionItems, terrainTile, sampleGround])

  const kinds = useMemo(() => kindsBySeq(lastPattern?.lines ?? []), [lastPattern])
  // Geoid offsets for the 3D-mesh datum: with the 3D tileset on, rendered
  // heights move to the ellipsoid datum (AMSL + N) so the trajectory hugs
  // the mesh; labels stay AMSL. Sampled per waypoint, cleared when 3D is
  // off (back to the AMSL plane).
  const [ellipsoidBySeq, setEllipsoidBySeq] = useState<Record<number, number> | null>(null)
  const meshOn = terrain3d != null
  useEffect(() => {
    if (!meshOn || missionItems.length === 0) {
      setEllipsoidBySeq(null)
      return
    }
    const id = setTimeout(() => {
      const points = missionItems
        .filter((it) => it.x !== 0 || it.y !== 0)
        .map((it) => [it.y / 1e7, it.x / 1e7] as [number, number])
      void (async () => {
        try {
          const { invoke } = await import('@tauri-apps/api/core')
          const samples = await invoke<Array<{ undulation_m: number | null }>>('geoid_sample', {
            points,
          })
          const next: Record<number, number> = {}
          missionItems.forEach((it, i) => {
            const n = samples[i]?.undulation_m
            if (n != null && (it.x !== 0 || it.y !== 0)) next[it.seq] = n
          })
          setEllipsoidBySeq(Object.keys(next).length > 0 ? next : null)
        } catch {
          setEllipsoidBySeq(null) // geoid unavailable: keep the AMSL plane
        }
      })()
    }, 300)
    return () => clearTimeout(id)
  }, [missionItems, meshOn])
  useEffect(() => {
    setMission(missionItems, selectedSeq, {
      kinds,
      heights: showHeights,
      // Ground plane for the sticks and the AGL chips: HOME's AMSL until a
      // DEM exists (WS-D) — the same plane the vehicle drop line and the AGL
      // compile mode stand on (0 until the plan carries a home position).
      // When the DEM is loaded, `groundBySeq` carries the DTM under each
      // point and takes precedence (accurate terrain-following markers).
      groundM: home?.[2] ?? 0,
      groundBySeq,
      ellipsoidBySeq,
      // Flight-state colouring: flown waypoints dim, the target gets a halo.
      currentSeq,
    })
  }, [
    missionItems,
    selectedSeq,
    kinds,
    showHeights,
    home,
    currentSeq,
    groundBySeq,
    ellipsoidBySeq,
    setMission,
  ])

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
