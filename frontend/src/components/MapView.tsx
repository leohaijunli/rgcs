import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useCesiumViewer } from '../hooks/useCesiumViewer'
import { FIELD_STALE_MS } from '../hooks/useDataAge'
import { compileWaypoints, toDisplayItems, itemLatLon } from '../mission/compile'
import { kindsBySeq } from '../mission/lineKinds'
import { orderedMissionItems } from '../mission/planfile'
import { usePrefsStore } from '../desktop/prefs'
import { useMissionStore } from '../stores/mission'
import { usePolygonStore } from '../stores/polygon'
import { useCommandStore } from '../stores/command'
import { useLinkStore } from '../stores/link'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore } from '../stores/ui'
import { useTerrainStore } from '../stores/terrain'
import { TRAIL_AGL_HI_M, TRAIL_AGL_LO_M } from '../cesium/constants'
import { TRAIL_BELOW_GROUND, TRAIL_RAMP } from '../cesium/trail'
import MapToolbar from './MapToolbar'

/** Live vehicle ground sampling: at most one sample per this interval, or
 * sooner once the vehicle has moved this far (metres). Keeps a 5–20 Hz fix
 * stream from hammering the terrain sampler while still tracking terrain. */
const GROUND_SAMPLE_MS = 1000
const GROUND_SAMPLE_MOVE_M = 25

/** N(EGM96) geoid undulation at a point (metres), or null when unavailable —
 * the vehicle-level counterpart of `ellipsoidBySeq` for the mesh datum. */
async function geoidAt(lat: number, lon: number): Promise<number | null> {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const s = await invoke<Array<{ undulation_m: number | null }>>('geoid_sample', {
      points: [[lat, lon]],
    })
    const u = s[0]?.undulation_m
    return u != null && Number.isFinite(u) ? u : null
  } catch {
    return null
  }
}

/** Composition shell: viewer lifecycle + store wiring + chrome (issues.md #29). */
export default function MapView() {
  const { t } = useTranslation()
  const containerRef = useRef<HTMLDivElement>(null)
  const [ctxMenu, setCtxMenu] = useState<{
    lat: number
    lon: number
    x: number
    y: number
  } | null>(null)
  const { initError, setSnapshot, setMeshOffset, setVehicleGround, setTrailByAgl, setMission, setTerrainOverlay, setTerrain3d, setSurveyCoverage, sampleMeshGround, setLayers, setImageryStyle, focusWaypoint, fitMission, goHome, lookNorth } =
    useCesiumViewer(containerRef, {
      onContextMenu: (ground, screen) =>
        setCtxMenu(ground ? { ...ground, x: screen.x, y: screen.y } : null),
    })
  const mapTool = useUiStore((s) => s.mapTool)
  const view = useUiStore((s) => s.view)
  const polygonClosed = usePolygonStore((s) => s.closed)
  const connected = useLinkStore((s) => Boolean(s.link?.fc_alive))
  const sendGoto = useCommandStore((s) => s.sendGoto)
  const sendSetHome = useCommandStore((s) => s.sendSetHome)

  // Close the context menu on an outside click or Esc.
  useEffect(() => {
    if (!ctxMenu) return
    const close = () => setCtxMenu(null)
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && close()
    window.addEventListener('pointerdown', close)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('pointerdown', close)
      window.removeEventListener('keydown', onKey)
    }
  }, [ctxMenu])

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
  const trailByAgl = useUiStore((s) => s.trailByAgl)
  useEffect(() => {
    setTrailByAgl(trailByAgl)
  }, [trailByAgl, setTrailByAgl])
  const showGrid = useUiStore((s) => s.showGrid)
  const imageryStyle = useUiStore((s) => s.imageryStyle)
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
  // U9b: outline the prepared DEM footprint (Layers toggle gates it).
  const coverage = useTerrainStore((st) => st.coverage)
  const showCoverage = useUiStore((s) => s.showCoverage)
  useEffect(() => {
    setSurveyCoverage(showCoverage ? coverage : null)
  }, [coverage, showCoverage, setSurveyCoverage])

  // 3D terrain tileset (plan B): react to the store's tileset base URL. A
  // provider load that fails (server gone, bad manifest) rolls the toggle
  // back through fail3d; a stale rejection after a newer switch is dropped.
  const terrain3d = useTerrainStore((st) => st.terrain3d)
  // Bumped each time the provider swap finished, so mesh sampling waits for it.
  const [meshEpoch, setMeshEpoch] = useState(0)
  useEffect(() => {
    let cancelled = false
    void setTerrain3d(terrain3d?.url ?? null)
      .then(() => {
        if (!cancelled) setMeshEpoch((n) => n + 1)
      })
      .catch((e) => {
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

  // The 3D mesh, once its provider swap settled, is the priority ground source
  // for every AGL readout (below it: the 2D DEM tile, then the HOME plane).
  const meshActive = terrain3d != null && meshEpoch > 0

  // Accurate AGL markers, fallback source: the 2D DEM tile, only while the
  // 3D mesh is not active (the mesh effect below takes precedence). The result
  // lives in the terrain store so the waypoint list shows the same ground;
  // debounced for drags.
  const terrainTile = useTerrainStore((st) => st.tile)
  const groundBySeq = useTerrainStore((st) => st.groundBySeq)
  const sampleGround = useTerrainStore((st) => st.sampleGround)
  const sampleDem = useTerrainStore((st) => st.sampleDem)
  useEffect(() => {
    // The 3D mesh (when active) owns `groundBySeq`; no tile either → nothing.
    if (!terrainTile || meshActive) return
    if (missionItems.length === 0) {
      useTerrainStore.setState({ groundBySeq: null })
      return
    }
    const id = setTimeout(() => {
      // Degrees, in the (lat, lon) order every sampler takes; command items
      // (DO_*) have no coordinate and are skipped.
      const points: Array<[number, number, number]> = []
      missionItems.forEach((it) => {
        const ll = itemLatLon(it)
        if (ll) points.push([it.seq, ll[0], ll[1]])
      })
      void sampleGround(points)
    }, 300)
    return () => clearTimeout(id)
  }, [missionItems, terrainTile, meshActive, sampleGround])

  const kinds = useMemo(() => kindsBySeq(lastPattern?.lines ?? []), [lastPattern])
  // Geoid offsets for the 3D-mesh datum: with the 3D tileset on, rendered
  // heights move to the ellipsoid datum (AMSL + N) so the trajectory hugs
  // the mesh; labels stay AMSL. Sampled per waypoint, cleared when 3D is
  // off (back to the AMSL plane).
  const [ellipsoidBySeq, setEllipsoidBySeq] = useState<Record<number, number> | null>(null)
  // Only add N(EGM96) when the tileset really is on the ellipsoid (built with
  // `--geoid`). A tileset built without it serves CGVD2013 H as if it were h,
  // and adding N (≈ −19.5 m at Sidney) would sink the trajectory below it.
  // `null` = not known yet (no mesh, or layer.json still in flight): the drape
  // waits for the datum rather than measuring against the wrong surface.
  const [meshDatum, setMeshDatum] = useState<'ellipsoidWgs84' | 'orthometric' | null>(null)
  const [meshBounds, setMeshBounds] = useState<[number, number, number, number] | null>(null)
  useEffect(() => {
    if (!terrain3d) {
      setMeshDatum(null)
      setMeshBounds(null)
      return
    }
    let cancelled = false
    fetch(`${terrain3d.url}layer.json`)
      .then((r) => r.json())
      .then((j: { verticalDatum?: string; bounds?: number[] }) => {
        if (cancelled) return
        setMeshDatum(j.verticalDatum === 'ellipsoidWgs84' ? 'ellipsoidWgs84' : 'orthometric')
        setMeshBounds(Array.isArray(j.bounds) && j.bounds.length === 4 ? (j.bounds as [number, number, number, number]) : null)
      })
      .catch(() => {
        // Manifest unreadable: assume the orthometric default (the same
        // assumption the rendering path makes) instead of blocking the mesh.
        if (!cancelled) setMeshDatum('orthometric')
      })
    return () => {
      cancelled = true
    }
  }, [terrain3d])
  const meshEllipsoid = meshDatum === 'ellipsoidWgs84'
  const meshOn = meshEllipsoid
  // One geoid offset at HOME (N varies ~cm per km) for the vehicle-level
  // layers — drone, trail, predict, HOME, drop line, ghost, polygon — so they
  // share the datum the plan already moved to. 0 when the mesh is off.
  useEffect(() => {
    if (!meshOn) {
      setMeshOffset(0)
      return
    }
    let cancelled = false
    void (async () => {
      try {
        const at = home ? { lat: home[0], lon: home[1] } : usePrefsStore.getState().initialPosition
        const { invoke } = await import('@tauri-apps/api/core')
        const samples = await invoke<Array<{ undulation_m: number | null }>>('geoid_sample', {
          points: [[at.lat, at.lon]],
        })
        if (!cancelled) setMeshOffset(samples[0]?.undulation_m ?? 0)
      } catch {
        if (!cancelled) setMeshOffset(0) // geoid unavailable: keep the AMSL plane
      }
    })()
    return () => {
      cancelled = true
    }
  }, [meshOn, home, setMeshOffset])
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
  // Priority ground source: with the 3D mesh active take the ground under each
  // waypoint from the mesh itself (even when a 2D tile is also loaded), so the
  // AGL chips and the list show real terrain clearance. Heights come back in
  // the tileset's frame; on an ellipsoid mesh remove N to get AMSL.
  useEffect(() => {
    if (!meshActive) {
      // The 2D effect above owns `groundBySeq`; clear a stale value when there
      // is no tile either.
      if (!terrainTile) useTerrainStore.setState({ groundBySeq: null })
      return
    }
    if (missionItems.length === 0) {
      useTerrainStore.setState({ groundBySeq: null })
      return
    }
    if (meshEllipsoid && !ellipsoidBySeq) return // N not sampled yet
    let cancelled = false
    const id = setTimeout(() => {
      const points: Array<[number, number, number]> = []
      missionItems.forEach((it) => {
        const ll = itemLatLon(it)
        if (ll) points.push([it.seq, ll[0], ll[1]])
      })
      sampleMeshGround(
        points.map(([, lat, lon]) => [lat, lon] as [number, number]),
        meshBounds,
      )
        .then(async (heights) => {
          if (cancelled) return
          const next: Record<number, number> = {}
          points.forEach(([seq], i) => {
            const h = heights[i]
            if (h != null) next[seq] = h - (meshEllipsoid ? (ellipsoidBySeq?.[seq] ?? 0) : 0)
          })
          // The mesh answered nothing for the whole set (bounds/datum gap):
          // fall back to the 2D DEM tile instead of reporting "no terrain".
          if (Object.keys(next).length === 0 && terrainTile) {
            const dtm = await sampleDem(points.map(([, lat, lon]) => [lat, lon]))
            points.forEach(([seq], i) => {
              if (dtm[i] != null) next[seq] = dtm[i] as number
            })
          }
          useTerrainStore.setState({ groundBySeq: Object.keys(next).length > 0 ? next : null })
        })
        .catch(() => {
          if (!cancelled) useTerrainStore.setState({ groundBySeq: null })
        })
    }, 300)
    return () => {
      cancelled = true
      clearTimeout(id)
    }
  }, [terrainTile, meshActive, meshBounds, meshEllipsoid, ellipsoidBySeq, missionItems, sampleMeshGround, sampleDem])

  // Ground under the FC HOME (the point the vehicle armed on), DEM/mesh
  // first. Anchors the live vehicle into the plan's AMSL frame
  // (`vehicleAmslAnchoredM`): PX4's own AMSL anchor is only as good as the
  // GNSS MSL fix at arming — measured ~15 m low on a BC site (takeoff AGL
  // −14 m), while `relative_alt` is exact against the same origin. The HOME
  // position never moves in flight, so this samples once per HOME fix.
  const homeGroundRef = useRef<{ lat: number; lon: number } | null>(null)
  const homePos = snapshot?.home
  useEffect(() => {
    if (!homePos || (!meshActive && !terrainTile)) {
      if (homeGroundRef.current !== null) {
        homeGroundRef.current = null
        useTerrainStore.setState({ homeGroundAmsl: null })
      }
      return
    }
    const lat = homePos.latitude_deg
    const lon = homePos.longitude_deg
    const prev = homeGroundRef.current
    if (prev && prev.lat === lat && prev.lon === lon) return
    homeGroundRef.current = { lat, lon }
    void (async () => {
      let ground: number | null = null
      if (meshActive) {
        try {
          const [h] = await sampleMeshGround([[lat, lon]], meshBounds)
          if (h != null) {
            const n = meshEllipsoid ? await geoidAt(lat, lon) : 0
            ground = n != null ? h - n : null
          }
        } catch {
          ground = null
        }
      }
      if (ground == null && terrainTile) {
        const [g] = await sampleDem([[lat, lon]])
        ground = g ?? null
      }
      useTerrainStore.setState({ homeGroundAmsl: ground })
    })()
  }, [homePos, meshActive, terrainTile, meshBounds, meshEllipsoid, sampleMeshGround, sampleDem])

  // The terrain drape prefers the same surface the map draws, but it runs in
  // the mission store and cannot reach the Cesium viewer: publish a sampler
  // while the mesh is live (and its vertical datum is known) so terrain mode
  // can measure the survey AGL against the mesh instead of only the 2D tile.
  useEffect(() => {
    if (!meshActive || !meshDatum) {
      useTerrainStore.setState({ meshSampler: null })
      return
    }
    useTerrainStore.setState({
      meshSampler: {
        verticalDatum: meshDatum,
        sample: (points) => sampleMeshGround(points, meshBounds),
      },
    })
    return () => useTerrainStore.setState({ meshSampler: null })
  }, [meshActive, meshDatum, meshBounds, sampleMeshGround])

  // Ground under the live vehicle for the AGL drop line / readout: the 3D
  // mesh first, then the 2D DEM tile, else the HOME plane (null). Throttled.
  const lastGroundSampleRef = useRef<{ t: number; lat: number; lon: number } | null>(null)
  useEffect(() => {
    const pos = snapshot?.global_position
    if (!pos) return
    if (!meshActive && !terrainTile) {
      setVehicleGround(null)
      return
    }
    const now = performance.now()
    const prev = lastGroundSampleRef.current
    const movedM = prev
      ? Math.hypot(
          (pos.latitude_deg - prev.lat) * 111_320,
          (pos.longitude_deg - prev.lon) *
            111_320 *
            Math.cos((pos.latitude_deg * Math.PI) / 180),
        )
      : Infinity
    if (prev && now - prev.t < GROUND_SAMPLE_MS && movedM < GROUND_SAMPLE_MOVE_M) return
    lastGroundSampleRef.current = { t: now, lat: pos.latitude_deg, lon: pos.longitude_deg }
    const { latitude_deg: lat, longitude_deg: lon } = pos
    void (async () => {
      let ground: number | null = null
      if (meshActive) {
        try {
          const [h] = await sampleMeshGround([[lat, lon]], meshBounds)
          if (h != null) {
            // Heights come back in the tileset frame; on an ellipsoid mesh
            // remove N to land on the AMSL the plan uses.
            const n = meshEllipsoid ? await geoidAt(lat, lon) : 0
            ground = n != null ? h - n : null
          }
        } catch {
          ground = null
        }
      }
      if (ground == null && terrainTile) {
        const [g] = await sampleDem([[lat, lon]])
        ground = g ?? null
      }
      setVehicleGround(ground)
      // Mirror into the terrain store: the HUD's AGL(DTM) reads this instead
      // of running its own sampler — one ground reference, never disagreeing.
      useTerrainStore.setState({ vehicleGroundAmslM: ground })
      // Landed self-calibration (2026-10-18 field report): the altimeter's
      // anchor error is a constant bias, so `ground − reported AMSL` captured
      // while ON_GROUND fixes the whole flight — takeoff at A, landing at B,
      // different elevations: the DEM under the current position cancels the
      // bias wherever the vehicle is. Preferred over the HOME-ground anchor,
      // which needs a HOME_POSITION fix PX4 does not always stream.
      const landed = snapshot?.extended_sys_state?.landed_state === 'on_ground'
      const landedAge = snapshot?.field_ages.extended_sys_state_at_ms
      const landedFresh =
        landed && (landedAge == null || Date.now() - landedAge <= FIELD_STALE_MS)
      if (landedFresh && ground != null && Number.isFinite(pos.altitude.meters)) {
        const offset = ground - pos.altitude.meters
        const prev = useTerrainStore.getState().amslAnchorOffsetM
        if (prev == null || Math.abs(prev - offset) > 0.25) {
          useTerrainStore.setState({ amslAnchorOffsetM: offset })
        }
      }
    })()
  }, [snapshot, meshActive, terrainTile, meshBounds, meshEllipsoid, sampleMeshGround, sampleDem, setVehicleGround])

  useEffect(() => {
    setMission(missionItems, selectedSeq, {
      kinds,
      heights: showHeights,
      // Ground plane for the sticks and the AGL chips: HOME's AMSL until a
      // DEM exists (WS-D) — the same plane the vehicle drop line and the AGL
      // compile mode stand on (0 until the plan carries a home position).
      // When the DEM is loaded, `groundBySeq` carries the DTM under each
      // point and takes precedence (accurate terrain-following markers).
      // `null` = the plan carries no HOME altitude (0 = unset): there is no
      // flat plane to measure from, so the chips show AMSL alone.
      groundM: home && home[2] !== 0 ? home[2] : null,
      groundBySeq,
      heightsRelativeToUnknownHome:
        !(home && home[2] !== 0) && (altitudeMode === 'relative' || altitudeMode === 'agl'),
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
    altitudeMode,
    currentSeq,
    groundBySeq,
    ellipsoidBySeq,
    setMission,
  ])

  // Generate bumps fitRequest: frame the freshly created pattern (#review).
  const fitRequest = useUiStore((s) => s.fitRequest)
  useEffect(() => {
    if (fitRequest > 0) fitMission()
  }, [fitRequest, fitMission])

  // Selecting a waypoint in the list flies the camera to it (#39). The map
  // keeps its own drag-preview selection; re-focusing on the same seq is a
  // no-op, so a drag commit does not yank the camera.
  const lastFocusedSeq = useRef<number | null>(null)
  useEffect(() => {
    if (selectedSeq === null || selectedSeq === lastFocusedSeq.current) return
    lastFocusedSeq.current = selectedSeq
    focusWaypoint(selectedSeq)
  }, [selectedSeq, focusWaypoint])

  // Tool shortcuts (P3 §4.3): W = add waypoint, P = draw polygon. Planning
  // only; Esc already returns to select (cesium/waypoints.ts).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (view !== 'plan') return
      const tag = (e.target as HTMLElement | null)?.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return
      if (e.metaKey || e.ctrlKey || e.altKey) return
      if (e.key === 'w' || e.key === 'W') {
        useUiStore.getState().setMapTool(useUiStore.getState().mapTool === 'add' ? 'select' : 'add')
      } else if (e.key === 'p' || e.key === 'P') {
        useUiStore.getState().setMapTool(
          useUiStore.getState().mapTool === 'polygon' ? 'select' : 'polygon',
        )
      } else if (e.key === 'c' || e.key === 'C') {
        useUiStore.getState().setMapTool(
          useUiStore.getState().mapTool === 'center' ? 'select' : 'center',
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
      {view === 'plan' && mapTool === 'add' && (
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded border border-line bg-panel/85 px-3 py-1 text-xs text-ink shadow-lg">
          {t('map.addHint')}
        </div>
      )}
      {view === 'plan' && mapTool === 'polygon' && !polygonClosed && (
        // Transient drawing hint (top-centre slot, §4.3); the boundary
        // readout lives in the plan panel's Area step (P2).
        <div className="pointer-events-none absolute left-1/2 top-3 -translate-x-1/2 rounded border border-line bg-panel/85 px-3 py-1 text-xs text-ink shadow-lg">
          {t('map.polygonHint')}
        </div>
      )}
      {trailByAgl && <TrailLegend />}
      <CoordinateReadout />
      {ctxMenu && (
        <div
          className="absolute z-20 w-44 rounded border border-line bg-panel p-1 shadow-xl"
          // The window-level pointerdown handler above closes the menu on any
          // press. Without this, pressing a menu button unmounts the menu
          // before the click lands, so the buttons never fire.
          onPointerDown={(e) => e.stopPropagation()}
          style={{
            left: Math.min(ctxMenu.x, window.innerWidth - 190),
            top: Math.min(ctxMenu.y, window.innerHeight - 96),
          }}
        >
          <div className="mono truncate px-1.5 py-0.5 text-[10px] text-muted">
            {ctxMenu.lat.toFixed(6)} · {ctxMenu.lon.toFixed(6)}
          </div>
          <button
            disabled={!connected}
            title={connected ? undefined : t('map.needLink')}
            onClick={() => {
              void sendGoto(ctxMenu.lat, ctxMenu.lon)
              setCtxMenu(null)
            }}
            className="w-full rounded px-2 py-1.5 text-left text-sm text-ink hover:bg-canvas disabled:opacity-40"
          >
            {t('map.gotoHere')}
          </button>
          <button
            disabled={!connected}
            title={connected ? undefined : t('map.needLink')}
            onClick={() => {
              void sendSetHome(ctxMenu.lat, ctxMenu.lon)
              setCtxMenu(null)
            }}
            className="w-full rounded px-2 py-1.5 text-left text-sm text-ink hover:bg-canvas disabled:opacity-40"
          >
            {t('map.setHomeHere')}
          </button>
        </div>
      )}
    </div>
  )
}

/** Vehicle position readout, bottom-right corner (§4.3: moved out of the
 * toolbar so the strip carries only tools).
 *
 * The map's own "old data" mark (L3) lives here as well: the marker on the
 * globe is relabelled too, but a chip survives the vehicle being off-screen or
 * behind terrain, and it is what the checks can assert on. */
function CoordinateReadout() {
  const { t } = useTranslation()
  const pos = useTelemetryStore((s) => s.snapshot?.global_position ?? null)
  const link = useLinkStore((s) => s.link)
  if (!pos) return null
  const stale = link !== null && !link.fc_alive
  return (
    <div className="mono pointer-events-none absolute bottom-3 right-3 rounded border border-line bg-panel/85 px-2 py-1 text-right text-[10px] leading-tight text-muted shadow-lg">
      {stale && (
        <div
          data-testid="map-stale"
          className="mb-0.5 rounded bg-warn px-1 py-px text-center font-medium text-canvas"
        >
          {t('map.uavStale')}
        </div>
      )}
      {pos.latitude_deg.toFixed(5)} · {pos.longitude_deg.toFixed(5)}
      <br />
      {Math.round(pos.altitude.meters)} m
    </div>
  )
}

/** Colour key for the flown track (shown while "track colour by height" is on). */
function TrailLegend() {
  const { t } = useTranslation()
  const gradient = `linear-gradient(to right, ${TRAIL_RAMP.map(([stop, css]) => `${css} ${stop * 100}%`).join(', ')})`
  return (
    <div className="mono pointer-events-none absolute bottom-12 right-3 w-36 rounded border border-line bg-panel/85 px-2 py-1 text-[10px] leading-tight text-muted shadow-lg">
      <div>{t('map.trailLegend')}</div>
      <div className="mt-1 h-1.5 rounded" style={{ background: gradient }} />
      <div className="mt-0.5 flex justify-between">
        <span>{TRAIL_AGL_LO_M}</span>
        <span>{TRAIL_AGL_HI_M}+</span>
      </div>
      <div className="mt-0.5 flex items-center gap-1">
        <span className="inline-block h-1.5 w-3 rounded" style={{ background: TRAIL_BELOW_GROUND }} />
        <span>&lt; 0</span>
      </div>
    </div>
  )
}
