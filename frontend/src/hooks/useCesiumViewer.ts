// React lifecycle binding for the Cesium map: create the viewer, its layers and
// interaction handlers on mount, tear everything down on unmount. `MapView`
// composes this hook and feeds it store data; it owns no app state itself
// (issues.md #29).

import { useCallback, useEffect, useRef, useState } from 'react'
import type { RefObject } from 'react'
import * as Cesium from 'cesium'
import 'cesium/Build/Cesium/Widgets/widgets.css'
import { applyLayerVisibility, createViewer } from '../cesium/scene'
import {
  createDroneLayer,
  createGhostPoint,
  createPolygonLayer,
  disposePolygonLayer,
  disposeWaypointLayer,
  previewWaypoint,
  renderWaypoints,
  setGhostPoint,
  updateDropLine,
  updatePolygonLayer,
} from '../cesium/entities'
import type {
  DroneLayer,
  GhostPoint,
  PolygonLayer,
  WaypointLayer,
  WaypointRenderOptions,
} from '../cesium/entities'
import { installMapTools } from '../cesium/waypoints'
import { createFollowController } from '../cesium/follow'
import type { FollowController } from '../cesium/follow'
import {
  FOCUS_FLIGHT_S,
  FOCUS_HEIGHT_M,
  PREDICT_HORIZON_S,
  PREDICT_MIN_GROUNDSPEED_M_S,
  PREDICT_STEP_S,
  TRAIL_EVERY,
  TRAIL_MAX,
  UAV_MODEL_NOSE_YAW_OFFSET_DEG,
} from '../cesium/constants'
import { groundSpeedMps, projectAhead, uavOrientation } from '../cesium/uav'
import { DEFAULT_ALT_AGL_M } from '../mission/compile'
import { usePrefsStore } from '../desktop/prefs'
import { usePolygonStore } from '../stores/polygon'
import { useUiStore } from '../stores/ui'
import { degFromMavInt, useMissionStore } from '../stores/mission'
import type { MissionItem } from '../generated-types/MissionItem'
import type { TelemetrySnapshot } from '../generated-types/TelemetrySnapshot'

export interface CesiumViewerHandle {
  /** Set when the map could not initialise (e.g. no WebGL). */
  initError: string | null
  /** Apply a telemetry snapshot: marker, attitude, trail and projection. */
  setSnapshot(snapshot: TelemetrySnapshot | null): void
  /** Re-render the mission waypoint layer. */
  setLayers(showImagery: boolean, showGrid: boolean): void
  setMission(
    items: MissionItem[],
    selectedSeq: number | null,
    opts?: WaypointRenderOptions,
  ): void
  /** Frame the home area and stop following. */
  goHome(): void
  /** Rotate the camera to north-up (no-op in 2D) and stop following. */
  lookNorth(): void
}

export function useCesiumViewer(containerRef: RefObject<HTMLDivElement | null>): CesiumViewerHandle {
  const [initError, setInitError] = useState<string | null>(null)
  const viewerRef = useRef<Cesium.Viewer | null>(null)
  const droneRef = useRef<DroneLayer | null>(null)
  const followRef = useRef<FollowController | null>(null)
  const trailPos = useRef<Cesium.Cartesian3[]>([])
  const predictPos = useRef<Cesium.Cartesian3[]>([])
  const tickRef = useRef(0)
  const wpLayerRef = useRef<WaypointLayer | null>(null)
  const wpHandlerRef = useRef<Cesium.ScreenSpaceEventHandler | null>(null)
  const ghostRef = useRef<GhostPoint | null>(null)
  const wpItemsRef = useRef<MissionItem[]>([])
  const homeAmslRef = useRef(0)
  const polygonLayerRef = useRef<PolygonLayer | null>(null)

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
      () => trailPos.current,
      () => predictPos.current,
      usePrefsStore.getState().initialPosition,
      useMissionStore.getState().home?.[2] ?? 0,
    )
    droneRef.current = layer
    const ghost = createGhostPoint(viewer)
    ghostRef.current = ghost
    const polygonLayer = createPolygonLayer(viewer)
    polygonLayerRef.current = polygonLayer
    wpHandlerRef.current = installMapTools(viewer, {
      // Waypoints are only editable in the planning view (finding 18).
      enabled: () => useUiStore.getState().view === 'planning',
      tool: () => useUiStore.getState().mapTool,
      onPreview: (seq, lat, lon) => {
        const layer = wpLayerRef.current
        if (layer) previewWaypoint(layer, wpItemsRef.current, seq, lat, lon)
      },
      onSelect: (seq) => useMissionStore.getState().select(seq),
      onAdd: (lat, lon) => useMissionStore.getState().addWaypointAt(lat, lon),
      onHover: (ground) => {
        // Preview the new waypoint where it will land: at the inherited
        // altitude (last waypoint, else home + default AGL), so the click
        // placement matches the rendered point even from a tilted camera.
        const st = useMissionStore.getState()
        const last = st.waypoints[st.waypoints.length - 1]
        const topM = last ? last.altitude.meters : homeAmslRef.current + DEFAULT_ALT_AGL_M
        setGhostPoint(ghost, ground, topM, homeAmslRef.current)
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

    return () => {
      removeCenterListener()
      follow.detach()
      wpHandlerRef.current?.destroy()
      wpHandlerRef.current = null
      disposeWaypointLayer(viewer, wpLayerRef.current)
      wpLayerRef.current = null
      disposePolygonLayer(viewer, polygonLayerRef.current)
      polygonLayerRef.current = null
      ghostRef.current = null
      viewer.destroy()
      viewerRef.current = null
      droneRef.current = null
      followRef.current = null
      trailPos.current = []
      predictPos.current = []
    }
  }, [containerRef])

  // Keep the scene's ground plane (and the HOME marker on it) in step with the
  // store's home altitude: the plan is drawn in AMSL, so the vehicle and the
  // drop line need the same reference to sit on the drawn path.
  const home = useMissionStore((s) => s.home)
  useEffect(() => {
    const amsl = home?.[2] ?? 0
    homeAmslRef.current = amsl
    const layer = droneRef.current
    const viewer = viewerRef.current
    if (layer && viewer && home) {
      layer.home.position = new Cesium.ConstantPositionProperty(
        Cesium.Cartesian3.fromDegrees(home[1], home[0], amsl),
      )
    }
  }, [home])

  // Map tool cursor: crosshair while adding, hidden ghost otherwise.
  const mapTool = useUiStore((s) => s.mapTool)
  const view = useUiStore((s) => s.view)

  // Draft survey polygon: re-render the boundary and its vertices whenever the
  // draft changes, only while the planning view is showing (view isolation).
  const polygonVertices = usePolygonStore((s) => s.vertices)
  const polygonClosed = usePolygonStore((s) => s.closed)
  useEffect(() => {
    const viewer = viewerRef.current
    const layer = polygonLayerRef.current
    if (!viewer || !layer) return
    updatePolygonLayer(viewer, layer, view === 'planning' ? polygonVertices : [], polygonClosed)
  }, [polygonVertices, polygonClosed, view])

  useEffect(() => {
    const viewer = viewerRef.current
    if (!viewer) return
    const editing = view === 'planning'
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

  const setSnapshot = useCallback((snapshot: TelemetrySnapshot | null) => {
    const layer = droneRef.current
    const pos = snapshot?.global_position
    if (!layer || !pos) return

    // Render the vehicle in the same frame the plan is drawn in (AMSL): the
    // waypoints, their labels and the height sticks are AMSL
    // (MapView.toDisplayItems), so a marker placed at PX4's relative_alt would
    // hang below the drawn path by the site's MSL elevation. Adding HOME's AMSL
    // puts the marker exactly on the path at the planned clearance. (Once a
    // DEM/terrain provider lands this becomes ellipsoid height + geoid.)
    const homeAmsl = homeAmslRef.current
    const droneAmsl = homeAmsl + pos.relative_alt_m
    const cart = Cesium.Cartesian3.fromDegrees(pos.longitude_deg, pos.latitude_deg, droneAmsl)

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
    layer.droneOrientation.setValue(
      uavOrientation(
        cart,
        (attitude?.yaw_deg ?? pos.heading_deg) + UAV_MODEL_NOSE_YAW_OFFSET_DEG,
        attitude?.pitch_deg ?? 0,
        attitude?.roll_deg ?? 0,
      ),
    )
    layer.drone.show = true
    // Straight down to the ground plane, labelled with the height above it, so
    // the clearance is visible in the scene and not only in the HUD
    // (issues.md #40). Same AMSL datum as the vehicle marker above.
    updateDropLine(layer.drop, pos.longitude_deg, pos.latitude_deg, droneAmsl, homeAmsl)

    // Rebuild the forward projection from the live fix so the segment already
    // flown disappears and only the predicted track ahead is drawn.
    predictPos.current =
      groundSpeedMps(pos) >= PREDICT_MIN_GROUNDSPEED_M_S
        ? projectAhead(pos, PREDICT_HORIZON_S, PREDICT_STEP_S)
        : []

    tickRef.current += 1
    if (tickRef.current % TRAIL_EVERY === 0) {
      trailPos.current.push(cart)
      if (trailPos.current.length > TRAIL_MAX) {
        trailPos.current = trailPos.current.slice(-TRAIL_MAX)
      }
    }
  }, [])

  const setMission = useCallback(
    (
      items: MissionItem[],
      selectedSeq: number | null,
      opts: WaypointRenderOptions = {},
    ) => {
      wpItemsRef.current = items
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

  // Frame a waypoint the operator picked from a list. One-shot: the request is
  // cleared as soon as it is served, so camera control stays with the operator
  // (a map pick selects but never moves the view).
  const focusSeq = useMissionStore((s) => s.focusSeq)
  useEffect(() => {
    if (focusSeq === null) return
    const viewer = viewerRef.current
    const item = wpItemsRef.current.find((it) => it.seq === focusSeq)
    useMissionStore.getState().clearFocus()
    if (!viewer || !item || item.frame === 'mission') return
    useUiStore.getState().setFollow(false)
    viewer.camera.flyTo({
      destination: Cesium.Cartesian3.fromDegrees(
        degFromMavInt(item.y),
        degFromMavInt(item.x),
        item.z + FOCUS_HEIGHT_M,
      ),
      duration: FOCUS_FLIGHT_S,
    })
  }, [focusSeq])

  const setLayers = useCallback((showImagery: boolean, showGrid: boolean) => {
    const viewer = viewerRef.current
    if (!viewer) return
    applyLayerVisibility(viewer, showImagery, showGrid)
  }, [])

  return { initError, setSnapshot, setMission, setLayers, goHome, lookNorth }
}
