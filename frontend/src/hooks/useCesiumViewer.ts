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
  disposeWaypointLayer,
  previewWaypoint,
  renderWaypoints,
  setGhostPoint,
  updateDropLine,
} from '../cesium/entities'
import type {
  DroneLayer,
  GhostPoint,
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
import { usePrefsStore } from '../desktop/prefs'
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
    )
    droneRef.current = layer
    const ghost = createGhostPoint(viewer)
    ghostRef.current = ghost
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
      onHover: (ground) => setGhostPoint(ghost, ground),
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
      ghostRef.current = null
      viewer.destroy()
      viewerRef.current = null
      droneRef.current = null
      followRef.current = null
      trailPos.current = []
      predictPos.current = []
    }
  }, [containerRef])

  // Map tool cursor: crosshair while adding, hidden ghost otherwise.
  const mapTool = useUiStore((s) => s.mapTool)
  const view = useUiStore((s) => s.view)
  useEffect(() => {
    const viewer = viewerRef.current
    if (!viewer) return
    const editing = view === 'planning'
    viewer.scene.canvas.style.cursor = editing && mapTool === 'add' ? 'crosshair' : ''
    if (editing && mapTool === 'add') return
    if (ghostRef.current) setGhostPoint(ghostRef.current, null)
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

    // Render height above the home ground, not AMSL: the map has no terrain
    // provider, so the home marker sits on the ellipsoid. Using relative_alt
    // keeps a landed vehicle on the ground instead of floating by the site's
    // MSL elevation. Switch back to MSL once a DEM/terrain provider lands.
    const cart = Cesium.Cartesian3.fromDegrees(pos.longitude_deg, pos.latitude_deg, pos.relative_alt_m)

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
    // Straight down to the ground, labelled with the height above it, so the
    // clearance is visible in the scene and not only in the HUD (issues.md #40).
    updateDropLine(layer.drop, pos.longitude_deg, pos.latitude_deg, pos.relative_alt_m)

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
