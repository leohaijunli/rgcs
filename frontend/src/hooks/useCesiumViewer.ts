// React lifecycle binding for the Cesium map: create the viewer, its layers and
// interaction handlers on mount, tear everything down on unmount. `MapView`
// composes this hook and feeds it store data; it owns no app state itself
// (issues.md #29).

import { useCallback, useEffect, useRef, useState } from 'react'
import type { RefObject } from 'react'
import * as Cesium from 'cesium'
import {
  setTerrain3d as sceneSetTerrain3d,
  setTerrainOverlay as sceneSetTerrainOverlay,
  setSurveyCoverage as sceneSetSurveyCoverage,
} from '../cesium/scene'
import 'cesium/Build/Cesium/Widgets/widgets.css'
import { applyLayerVisibility, createViewer, setImageryStyle as applyImageryStyle } from '../cesium/scene'
import { flyToPattern, flyToWaypoint } from '../cesium/entities'
import {
  createCenterHandle,
  createDroneLayer,
  createGhostPoint,
  createPolygonLayer,
  disposePolygonLayer,
  disposeWaypointLayer,
  previewWaypoint,
  renderWaypoints,
  setDroneStale,
  setGhostPoint,
  updateCenterHandle,
  updateDropLine,
  updatePolygonLayer,
} from '../cesium/entities'
import type {
  CenterHandle,
  DroneLayer,
  GhostPoint,
  PolygonLayer,
  WaypointLayer,
  WaypointRenderOptions,
} from '../cesium/entities'
import { installMapTools } from '../cesium/waypoints'
import { createFollowController } from '../cesium/follow'
import { createTrailLayer } from '../cesium/trail'
import type { TrailLayer } from '../cesium/trail'
import type { FollowController } from '../cesium/follow'
import {
  PREDICT_HORIZON_S,
  PREDICT_MIN_GROUNDSPEED_M_S,
  PREDICT_STEP_S,
} from '../cesium/constants'
import { groundSpeedMps, projectAhead, uavOrientation } from '../cesium/uav'
import { DEFAULT_ALT_AGL_M } from '../mission/compile'
import { usePrefsStore } from '../desktop/prefs'
import { usePolygonStore } from '../stores/polygon'
import { useUiStore } from '../stores/ui'
import { useMissionStore } from '../stores/mission'
import { useCommandStore } from '../stores/command'
import { useTerrainStore } from '../stores/terrain'
import { useLinkStore } from '../stores/link'
import { groundReferenceAmsl, vehicleAmslAnchoredM } from '../cesium/heights'
import type { MissionItem } from '../generated-types/MissionItem'
import type { TelemetrySnapshot } from '../generated-types/TelemetrySnapshot'

/** How many extra sampling passes a mesh that answered nothing gets (right
 * after a provider swap or a camera move the tiles under the points are not
 * resolved yet, and the AGL readouts would say "unknown" next to a mesh that is
 * plainly on screen), and how long to wait between passes, milliseconds. */
const MESH_SAMPLE_RETRIES = 3
const MESH_SAMPLE_RETRY_MS = 900

export interface CesiumViewerHandle {
  /** Set when the map could not initialise (e.g. no WebGL). */
  initError: string | null
  /** Apply a telemetry snapshot: marker, attitude, trail and projection. */
  setSnapshot(snapshot: TelemetrySnapshot | null, record?: boolean): void
  /** Geoid offset N (m) added to every rendered height while the ellipsoid
   * 3D mesh is on, so the vehicle, trail, HOME and plan share one datum;
   * `0` when it is off. */
  setMeshOffset(offsetM: number): void
  /** DEM/mesh ground elevation (AMSL) under the live vehicle, or `null`
   * outside coverage — the ground the drop line and AGL readout stand on. */
  setVehicleGround(groundAmslM: number | null): void
  /** Re-render the mission waypoint layer. */
  setLayers(showGrid: boolean): void
  setMission(
    items: MissionItem[],
    selectedSeq: number | null,
    opts?: WaypointRenderOptions,
  ): void
  /** Swap the Google imagery layer (satellite / hybrid). */
  setImageryStyle(style: 'google-satellite' | 'google-hybrid'): void
  /** Camera flyTo a selected waypoint (#39): the list click shows the point. */
  focusWaypoint(seq: number): void
  /** Frame the whole mission (post-Generate overview). */
  fitMission(): void
  /** Add/remove the terrain hillshade overlay (plan A); `alpha` is the
   * operator's U16 opacity (defaults to the historical 0.55). */
  setTerrainOverlay(
    overlay: {
      url: string | null
      bounds: { north: number; south: number; east: number; west: number }
    } | null,
    alpha?: number,
  ): void
  /** U9b: outline the prepared DEM footprint, or clear it with `null`. */
  setSurveyCoverage(
    bounds: { north: number; south: number; east: number; west: number } | null,
  ): void
  /** Switch the 3D terrain provider to the local tileset URL, or back to
   * the ellipsoid with `null` (plan B). Rejects when the endpoint fails. */
  setTerrain3d(url: string | null): Promise<void>
  /** Ground height under each `[lat, lon]` taken from the served 3D mesh, in
   * the tileset's own frame (`null` outside `bounds` / without a mesh). */
  sampleMeshGround(
    points: Array<[number, number]>,
    bounds: [number, number, number, number] | null,
  ): Promise<Array<number | null>>
  /** Frame the home area and stop following. */
  goHome(): void
  /** Rotate the camera to north-up (no-op in 2D) and stop following. */
  lookNorth(): void
}

/** A right-click pick on the map, for the context menu. */
export interface MapContextMenu {
  lat: number
  lon: number
  /** Screen position of the right-click (CSS pixels), to anchor the menu. */
  screen: { x: number; y: number }
}

/** Options for `useCesiumViewer`. */
export interface CesiumViewerOptions {
  /** A right-click that was not a polygon-vertex removal: the picked ground
   * point and its screen position, so the map can show a context menu. */
  onContextMenu?: (ground: { lat: number; lon: number } | null, screen: { x: number; y: number }) => void
}

export function useCesiumViewer(
  containerRef: RefObject<HTMLDivElement | null>,
  opts: CesiumViewerOptions = {},
): CesiumViewerHandle {
  const [initError, setInitError] = useState<string | null>(null)
  const viewerRef = useRef<Cesium.Viewer | null>(null)
  const droneRef = useRef<DroneLayer | null>(null)
  const followRef = useRef<FollowController | null>(null)
  const trailRef = useRef<TrailLayer | null>(null)
  const lastSnapshotRef = useRef<TelemetrySnapshot | null>(null)
  const wpOptsRef = useRef<WaypointRenderOptions>({})
  const meshOffsetRef = useRef(0)
  const vehicleGroundRef = useRef<number | null>(null)
  const [meshOffset, setMeshOffsetState] = useState(0)
  const predictPos = useRef<Cesium.Cartesian3[]>([])
  const wpLayerRef = useRef<WaypointLayer | null>(null)
  const wpHandlerRef = useRef<Cesium.ScreenSpaceEventHandler | null>(null)
  const ghostRef = useRef<GhostPoint | null>(null)
  const wpItemsRef = useRef<MissionItem[]>([])
  const homeAmslRef = useRef(0)
  const polygonLayerRef = useRef<PolygonLayer | null>(null)
  const centerHandleRef = useRef<CenterHandle | null>(null)
  // Latest right-click callback (replaced on every render so the mount-time
  // `installMapTools` closure always calls the current handler).
  const onContextRef = useRef(opts.onContextMenu)
  useEffect(() => {
    onContextRef.current = opts.onContextMenu
  }, [opts.onContextMenu])

  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    let viewer: Cesium.Viewer
    try {
      viewer = createViewer(container, usePrefsStore.getState().initialPosition)
    } catch (e) {
      setInitError(e instanceof Error ? e.message : String(e))
      return
    }

    const layer = createDroneLayer(
      viewer,
      () => predictPos.current,
      usePrefsStore.getState().initialPosition,
      useMissionStore.getState().home?.[2] ?? 0,
    )
    droneRef.current = layer
    const trail = createTrailLayer(viewer)
    trailRef.current = trail
    const ghost = createGhostPoint(viewer)
    ghostRef.current = ghost
    // Render height = AMSL + mesh datum offset (0 unless the ellipsoid mesh is on).
    const rh = (amslM: number) => amslM + meshOffsetRef.current
    // Altitude a newly added waypoint would get: last waypoint, else home + default AGL.
    const ghostTopM = () => {
      const st = useMissionStore.getState()
      const last = st.waypoints[st.waypoints.length - 1]
      return last ? last.altitude.meters : homeAmslRef.current + DEFAULT_ALT_AGL_M
    }
    const polygonLayer = createPolygonLayer(viewer)
    polygonLayerRef.current = polygonLayer
    const centerHandle = createCenterHandle(viewer)
    centerHandleRef.current = centerHandle
    wpHandlerRef.current = installMapTools(viewer, {
      // Waypoints are only editable in the planning view (finding 18).
      enabled: () => useUiStore.getState().view === 'plan',
      tool: () => useUiStore.getState().mapTool,
      onPreview: (seq, lat, lon) => {
        const layer = wpLayerRef.current
        if (layer) previewWaypoint(layer, wpItemsRef.current, seq, lat, lon, wpOptsRef.current.ellipsoidBySeq ?? null)
      },
      onSelect: (seq) => useMissionStore.getState().select(seq),
      onAdd: (lat, lon) => useMissionStore.getState().addWaypointAt(lat, lon),
      onHover: (ground) => {
        // Preview the new waypoint where it will land: at the inherited
        // altitude (last waypoint, else home + default AGL), so the click
        // placement matches the rendered point even from a tilted camera.
        setGhostPoint(ghost, ground, rh(ghostTopM()), rh(homeAmslRef.current))
      },
      // Pick on the horizontal surface at the point's own height so it stays
      // under the cursor from a tilted camera.
      pickHeightM: (seq) => {
        if (seq === null) return rh(ghostTopM())
        const item = wpItemsRef.current.find((it) => it.seq === seq)
        if (!item) return null
        return item.z + (wpOptsRef.current.ellipsoidBySeq?.[seq] ?? 0)
      },
      // Terrain on: vertices are draped, so project them where they are drawn.
      polygonHeightM: (v) =>
        useTerrainStore.getState().terrain3d
          ? (viewer.scene.globe.getHeight(
              Cesium.Cartographic.fromDegrees(v.longitude_deg, v.latitude_deg),
            ) ?? 0)
          : rh(homeAmslRef.current),
      // Right-click context menu (move-to / set-home): works in both views.
      onContextMenu: (ground, screen) => {
        onContextRef.current?.(
          ground ? { lat: ground.lat, lon: ground.lon } : null,
          { x: screen.x, y: screen.y },
        )
      },
    })
    const follow = createFollowController(viewer, layer.dronePosition)
    follow.attach()
    followRef.current = follow
    viewerRef.current = viewer
    // Dev-only handle for inspecting the scene (e.g. via the DevTools protocol).
    if (import.meta.env.DEV) {
      ;(window as unknown as { __mgViewer?: Cesium.Viewer }).__mgViewer = viewer
    }

    // Track the map-camera centre so a new waypoint can be added where the user
    // is looking (issues.md #14).
    const publishCenter = () => {
      const c = viewer.camera.positionCartographic
      useUiStore.getState().setMapCenter({
        lat: Cesium.Math.toDegrees(c.latitude),
        lon: Cesium.Math.toDegrees(c.longitude),
      })
    }
    const removeCenterListener = viewer.camera.moveEnd.addEventListener(publishCenter)
    publishCenter()

    // A8: a WebGL context loss (driver reset, memory pressure) kills the
    // globe but not the app — DOM listeners cannot reach React's error
    // boundary, so the event goes through a custom event the MapErrorBoundary
    // subscribes to: it shows the map-only failure card while the flight
    // spine (TopBar, commands, alerts, the drape panel) keeps running.
    const onContextLost = (e: Event) => {
      e.preventDefault()
      window.dispatchEvent(
        new CustomEvent('maggcs:map-failed', { detail: 'WebGL context lost — the map needs a restart' }),
      )
    }
    viewer.scene.canvas.addEventListener('webglcontextlost', onContextLost)

    return () => {
      // Best-effort teardown (A8): after a WebGL context loss Cesium's
      // internals are already gone, so cleanup calls can throw — the context
      // is dead either way, and the boundary's retry mounts a fresh viewer.
      // The spine (stores, layers state) is React-owned and unaffected.
      try {
        viewer.scene.canvas.removeEventListener('webglcontextlost', onContextLost)
        removeCenterListener()
        follow.detach()
        wpHandlerRef.current?.destroy()
        disposeWaypointLayer(viewer, wpLayerRef.current)
        disposePolygonLayer(viewer, polygonLayerRef.current)
        trailRef.current?.dispose()
        viewer.destroy()
      } catch (e) {
        console.warn('[MapView] teardown after context loss:', e)
      }
      wpHandlerRef.current = null
      wpLayerRef.current = null
      centerHandleRef.current = null
      ghostRef.current = null
      viewerRef.current = null
      droneRef.current = null
      followRef.current = null
      trailRef.current = null
      predictPos.current = []
    }
  }, [containerRef])

  // Heartbeat lost (L3): the marker keeps the last fix, so it must not read as
  // the live position — relabel it "OLD DATA" (same shape, warn tone) until the
  // heartbeat is back. Outside React's render so the entity is updated in
  // place, once per transition.
  useEffect(() => {
    const apply = () => {
      const layer = droneRef.current
      if (layer) setDroneStale(layer, !(useLinkStore.getState().link?.fc_alive ?? true))
    }
    apply()
    return useLinkStore.subscribe(apply)
  }, [])

  // Keep the scene's ground plane (and the HOME marker on it) in step with the
  // store's home altitude: the plan is drawn in AMSL, so the vehicle and the
  // drop line need the same reference to sit on the drawn path.
  const home = useMissionStore((s) => s.home)
  const homeMeshOn = useTerrainStore((s) => s.terrain3d != null)
  // HOME the vehicle accepted via "Set home" wins over the plan's HOME for the
  // marker only; the plan's ground reference (homeAmslRef) is untouched.
  const vehicleHome = useCommandStore((s) => s.homeMarker)
  useEffect(() => {
    const amsl = home?.[2] ?? 0
    homeAmslRef.current = amsl
    const layer = droneRef.current
    const viewer = viewerRef.current
    const shown = vehicleHome ?? home
    if (layer && viewer && shown) {
      // Flat globe (no 3D mesh): HOME is on the ground, i.e. on the ellipsoid
      // the vehicle is drawn relative to (see setSnapshot).
      const homeH = homeMeshOn ? shown[2] + meshOffset : 0
      layer.home.position = new Cesium.ConstantPositionProperty(
        Cesium.Cartesian3.fromDegrees(shown[1], shown[0], homeH),
      )
    }
  }, [home, vehicleHome, meshOffset, homeMeshOn])

  // "Move to here" click marker.
  const gotoTarget = useCommandStore((s) => s.gotoTarget)
  useEffect(() => {
    const layer = droneRef.current
    if (!layer) return
    if (!gotoTarget) {
      layer.target.show = false
      return
    }
    layer.target.position = new Cesium.ConstantPositionProperty(
      Cesium.Cartesian3.fromDegrees(gotoTarget[1], gotoTarget[0], 0),
    )
    layer.target.show = true
  }, [gotoTarget])

  // Map tool cursor: crosshair while adding, hidden ghost otherwise.
  const mapTool = useUiStore((s) => s.mapTool)
  const view = useUiStore((s) => s.view)

  // Draft survey polygon: re-render the boundary and its vertices whenever the
  // draft changes, only while the planning view is showing (view isolation).
  const polygonVertices = usePolygonStore((s) => s.vertices)
  const polygonClosed = usePolygonStore((s) => s.closed)
  // With a terrain tileset loaded the draft boundary drapes onto it.
  const terrain3dOn = useTerrainStore((s) => s.terrain3d != null)
  useEffect(() => {
    const viewer = viewerRef.current
    const layer = polygonLayerRef.current
    if (!viewer || !layer) return
    updatePolygonLayer(
      viewer,
      layer,
      view === 'plan' ? polygonVertices : [],
      polygonClosed,
      homeAmslRef.current + meshOffset,
      terrain3dOn,
    )
  }, [polygonVertices, polygonClosed, view, meshOffset, home, terrain3dOn])

  // Pinned pattern centre: render the draggable handle in the planning view.
  const patternCenter = useMissionStore((s) => s.patternCenter)
  useEffect(() => {
    const handle = centerHandleRef.current
    if (!handle) return
    updateCenterHandle(
      handle,
      view === 'plan' ? patternCenter : null,
      homeAmslRef.current + meshOffset,
      terrain3dOn,
    )
  }, [patternCenter, view, meshOffset, home, terrain3dOn])

  useEffect(() => {
    const viewer = viewerRef.current
    if (!viewer) return
    const editing = view === 'plan'
    viewer.scene.canvas.style.cursor = editing && mapTool === 'add' ? 'crosshair' : ''
    if (editing && mapTool === 'add') return
    if (ghostRef.current) setGhostPoint(ghostRef.current, null, 0, 0)
  }, [mapTool, view])

  // Follow mode / 2D-3D mode.
  const map3d = useUiStore((s) => s.map3d)
  useEffect(() => {
    const viewer = viewerRef.current
    if (!viewer) return
    if (map3d) viewer.scene.morphTo3D(0)
    else viewer.scene.morphTo2D(0)
  }, [map3d])

  const setSnapshot = useCallback((snapshot: TelemetrySnapshot | null, record = true) => {
    const layer = droneRef.current
    const pos = snapshot?.global_position
    if (!layer || !pos) return
    lastSnapshotRef.current = snapshot
    const offset = meshOffsetRef.current

    // Drop the "Move to here" marker once the vehicle is within ~3 m of it.
    const tgt = useCommandStore.getState().gotoTarget
    if (tgt) {
      const dNorth = (pos.latitude_deg - tgt[0]) * 111_320
      const dEast = (pos.longitude_deg - tgt[1]) * 111_320 * Math.cos((tgt[0] * Math.PI) / 180)
      if (Math.hypot(dNorth, dEast) < 3) useCommandStore.getState().clearGotoTarget()
    }

    // Vehicle height in the frame the plan is drawn in (AMSL). Prefer the
    // AMSL PX4 reports (the HUD's AMSL readout): it does not depend on the
    // plan's HOME. `HOME + relative_alt` is the fallback only; with no plan
    // home it is `relative_alt` alone, which hangs the marker below the path
    // by the site elevation, and a HOME that changes in flight would step
    // the whole trail. (Once the 3D mesh is on the datum offset is added.)
    const reportedAmsl = pos.altitude.meters
    // Anchor the vehicle into the plan's AMSL frame first: `alt` is only as
    // good as the GNSS MSL fix at arming (measured ~15 m low on a BC site),
    // while `relative_alt` + the DEM ground under HOME is exact. Everything
    // below (AGL reference, drop line, trail) reads the anchored value.
    const anchoredAmsl = vehicleAmslAnchoredM({
      anchorOffsetM: useTerrainStore.getState().amslAnchorOffsetM,
      demGroundAtHomeM: useTerrainStore.getState().homeGroundAmsl,
      reportedAmslM: reportedAmsl,
      relativeAltM: pos.relative_alt_m,
    })
    // Ground reference for the AGL drop line / readout, by priority: the
    // DEM/mesh surface under the vehicle (sampled by MapView), else the plan's
    // HOME elevation, else PX4's own HOME (AMSL − relative_alt). Without the
    // DEM the label echoed the AMSL altitude (a hovering vehicle read its MSL
    // value as clearance). See `cesium/heights.ts`.
    const groundAmsl = vehicleGroundRef.current
    const homeAmsl = groundReferenceAmsl({
      planHomeAmslM: homeAmslRef.current,
      reportedAmslM: anchoredAmsl,
      relativeAltM: pos.relative_alt_m,
      demGroundAmslM: groundAmsl,
    })
    const droneAmsl = Number.isFinite(anchoredAmsl) ? anchoredAmsl : homeAmsl + pos.relative_alt_m
    // Without the 3D terrain mesh the globe is the bare ellipsoid (height 0),
    // so the visible ground is NOT at HOME's AMSL: a landed vehicle drawn at
    // AMSL (e.g. 4 m) hovers above the map. Draw it at its height above HOME
    // instead, so HOME / the ground sits on the globe. With the mesh on, keep
    // the AMSL + geoid-offset datum the plan uses.
    const meshOn = useTerrainStore.getState().terrain3d != null
    const useRel = !meshOn && Number.isFinite(pos.relative_alt_m)
    const renderAmsl = useRel ? pos.relative_alt_m : droneAmsl + offset
    // The flat globe's vertical origin is the home ground, so a DEM surface
    // has to be expressed relative to it (0 = the plane itself, no DEM).
    const vehicleHomeAmsl = droneAmsl - pos.relative_alt_m
    const groundRender = useRel
      ? groundAmsl != null && Number.isFinite(vehicleHomeAmsl)
        ? groundAmsl - vehicleHomeAmsl
        : 0
      : homeAmsl + offset
    const cart = Cesium.Cartesian3.fromDegrees(pos.longitude_deg, pos.latitude_deg, renderAmsl)

    // Attitude straight from the MAVLink ATTITUDE message; fall back to the
    // reported heading until the first attitude sample arrives. The marker is a
    // world-space object, so orbiting the camera only changes the viewpoint.
    const attitude = snapshot?.attitude
    const enu = Cesium.Transforms.eastNorthUpToFixedFrame(cart)
    const vel = Cesium.Matrix4.multiplyByPointAsVector(
      enu,
      new Cesium.Cartesian3(pos.velocity.y_m_s, pos.velocity.x_m_s, -pos.velocity.z_m_s),
      new Cesium.Cartesian3(),
    )
    followRef.current?.setFix({ cart, vel, atMs: performance.now() })
    // Model orientation offsets (issues.md #43, Settings → Vehicle): the
    // yaw default keeps the asset-nose correction the constant used to
    // hard-code.
    const model = usePrefsStore.getState().modelOrientation
    layer.droneOrientation.setValue(
      uavOrientation(
        cart,
        (attitude?.yaw_deg ?? pos.heading_deg) + model.yaw,
        (attitude?.pitch_deg ?? 0) + model.pitch,
        (attitude?.roll_deg ?? 0) + model.roll,
      ),
    )
    layer.drone.show = true
    // Straight down to the ground plane, labelled with the height above it, so
    // the clearance is visible in the scene and not only in the HUD
    // (issues.md #40). Same datum as the vehicle marker above.
    updateDropLine(layer.drop, pos.longitude_deg, pos.latitude_deg, renderAmsl, groundRender)

    // Rebuild the forward projection from the live fix so the segment already
    // flown disappears and only the predicted track ahead is drawn. It starts
    // at the marker's own render height.
    predictPos.current =
      groundSpeedMps(pos) >= PREDICT_MIN_GROUNDSPEED_M_S
        ? projectAhead(pos, PREDICT_HORIZON_S, PREDICT_STEP_S, renderAmsl)
        : []

    // Record the track: AMSL for position. The trail bakes `offset` in at
    // render time; in the flat-globe case it must share the marker's
    // height-above-HOME frame (offset is 0 there).
    if (record) {
      const trailH = useRel ? pos.relative_alt_m : droneAmsl
      trailRef.current?.push(pos.longitude_deg, pos.latitude_deg, trailH)
    }
  }, [])

  // Datum offset changed (3D mesh toggled): re-render everything that bakes
  // the offset into a Cartesian — the recorded trail and the live marker.
  const setMeshOffset = useCallback((offsetM: number) => {
    const next = Number.isFinite(offsetM) ? offsetM : 0
    if (next === meshOffsetRef.current) return
    meshOffsetRef.current = next
    trailRef.current?.setOffset(next)
    setSnapshot(lastSnapshotRef.current, false)
    setMeshOffsetState(next)
  }, [setSnapshot])

  // DEM/mesh ground under the live vehicle changed (MapView sampled it):
  // re-render the marker so the drop line lands on the terrain and the AGL
  // label updates, without recording another trail point.
  const setVehicleGround = useCallback(
    (groundAmslM: number | null) => {
      const next = groundAmslM != null && Number.isFinite(groundAmslM) ? groundAmslM : null
      if (next === vehicleGroundRef.current) return
      vehicleGroundRef.current = next
      setSnapshot(lastSnapshotRef.current, false)
    },
    [setSnapshot],
  )

  const setMission = useCallback(
    (
      items: MissionItem[],
      selectedSeq: number | null,
      opts: WaypointRenderOptions = {},
    ) => {
      wpItemsRef.current = items
      wpOptsRef.current = opts
      const viewer = viewerRef.current
      if (!viewer) return
      disposeWaypointLayer(viewer, wpLayerRef.current)
      wpLayerRef.current = renderWaypoints(viewer, items, selectedSeq, opts)
    },
    [],
  )

  const goHome = useCallback(() => {
    const viewer = viewerRef.current
    if (!viewer) return
    // Home is a wide overview, so stop following or the next frame re-centres.
    useUiStore.getState().setFollow(false)
    viewer.camera.lookAtTransform(Cesium.Matrix4.IDENTITY)
    const home = usePrefsStore.getState().initialPosition
    viewer.camera.flyTo({ destination: Cesium.Cartesian3.fromDegrees(home.lon, home.lat, 12000) })
  }, [])

  const lookNorth = useCallback(() => {
    const viewer = viewerRef.current
    if (!viewer) return
    // North-up is a 3D orientation; in 2D the camera heading is fixed anyway.
    if (viewer.scene.mode !== Cesium.SceneMode.SCENE3D) return
    useUiStore.getState().setFollow(false)
    viewer.camera.setView({
      destination: viewer.camera.positionWC,
      orientation: {
        heading: 0,
        pitch: viewer.camera.pitch,
        roll: viewer.camera.roll,
      },
    })
  }, [])

  const setImageryStyle = useCallback((style: 'google-satellite' | 'google-hybrid') => {
    const viewer = viewerRef.current
    if (!viewer) return
    applyImageryStyle(viewer, style)
  }, [])

  const focusWaypoint = useCallback((seq: number) => {
    const viewer = viewerRef.current
    if (!viewer) return
    flyToWaypoint(viewer, wpItemsRef.current, seq, wpOptsRef.current.ellipsoidBySeq ?? null)
  }, [])

  const fitMission = useCallback(() => {
    const viewer = viewerRef.current
    if (!viewer) return
    flyToPattern(viewer, wpItemsRef.current)
  }, [])

  const setTerrainOverlay = useCallback(
    (
      overlay: {
        url: string | null
        bounds: { north: number; south: number; east: number; west: number }
      } | null,
      alpha?: number,
    ) => {
      const viewer = viewerRef.current
      if (!viewer) return
      sceneSetTerrainOverlay(viewer, overlay, alpha)
    },
    [],
  )

  const sampleMeshGround = useCallback(
    async (
      points: Array<[number, number]>,
      bounds: [number, number, number, number] | null,
    ): Promise<Array<number | null>> => {
      const out: Array<number | null> = points.map(() => null)
      const viewer = viewerRef.current
      if (!viewer || viewer.isDestroyed()) return out
      const provider = viewer.terrainProvider
      // Only a real mesh: the ellipsoid provider would answer 0 everywhere.
      if (!(provider instanceof Cesium.CesiumTerrainProvider)) return out
      // Outside the DEM the globe stays flat; do not report that as ground.
      const covered: number[] = []
      points.forEach(([lat, lon], i) => {
        const inside =
          !bounds || (lon >= bounds[0] && lon <= bounds[2] && lat >= bounds[1] && lat <= bounds[3])
        if (inside) covered.push(i)
      })
      if (covered.length === 0) return out
      const once = async (): Promise<void> => {
        const sampled = await Cesium.sampleTerrainMostDetailed(
          provider,
          covered.map((i) => Cesium.Cartographic.fromDegrees(points[i][1], points[i][0])),
        )
        sampled.forEach((c, k) => {
          if (Number.isFinite(c.height)) out[covered[k]] = c.height
        })
      }
      await once()
      for (
        let attempt = 0;
        attempt < MESH_SAMPLE_RETRIES && out.every((h) => h == null) && !viewer.isDestroyed();
        attempt += 1
      ) {
        await new Promise((resolve) => setTimeout(resolve, MESH_SAMPLE_RETRY_MS))
        if (viewer.isDestroyed()) break
        await once()
      }
      return out
    },
    [],
  )

  const setSurveyCoverage = useCallback(
    (bounds: { north: number; south: number; east: number; west: number } | null) => {
      const viewer = viewerRef.current
      if (!viewer) return
      sceneSetSurveyCoverage(viewer, bounds)
    },
    [],
  )

  const setTerrain3d = useCallback(async (url: string | null) => {
    const viewer = viewerRef.current
    if (!viewer) return
    await sceneSetTerrain3d(viewer, url)
  }, [])

  const setLayers = useCallback((showGrid: boolean) => {
    const viewer = viewerRef.current
    if (!viewer) return
    applyLayerVisibility(viewer, showGrid)
  }, [])

  return { initError, setSnapshot, setMeshOffset, setVehicleGround, setMission, setTerrainOverlay, setTerrain3d, setSurveyCoverage, sampleMeshGround, setLayers, setImageryStyle, focusWaypoint, fitMission, goHome, lookNorth }
}
