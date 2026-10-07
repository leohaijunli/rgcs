// React lifecycle binding for the Cesium map: create the viewer, its layers and
// interaction handlers on mount, tear everything down on unmount. `MapView`
// composes this hook and feeds it store data; it owns no app state itself
// (issues.md #29).

import { useCallback, useEffect, useRef, useState } from 'react'
import type { RefObject } from 'react'
import * as Cesium from 'cesium'
import 'cesium/Build/Cesium/Widgets/widgets.css'
import { createViewer } from '../cesium/scene'
import { createDroneLayer, disposeWaypointLayer, renderWaypoints } from '../cesium/entities'
import type { DroneLayer, WaypointLayer } from '../cesium/entities'
import { installWaypointDrag } from '../cesium/waypoints'
import { createFollowController } from '../cesium/follow'
import type { FollowController } from '../cesium/follow'
import {
  HOME_LAT,
  HOME_LON,
  PREDICT_HORIZON_S,
  PREDICT_MIN_GROUNDSPEED_M_S,
  PREDICT_STEP_S,
  TRAIL_EVERY,
  TRAIL_MAX,
  UAV_MODEL_NOSE_YAW_OFFSET_DEG,
} from '../cesium/constants'
import { groundSpeedMps, projectAhead, uavOrientation } from '../cesium/uav'
import { useUiStore } from '../stores/ui'
import type { MissionItem } from '../generated-types/MissionItem'
import type { TelemetrySnapshot } from '../generated-types/TelemetrySnapshot'

export interface CesiumViewerHandle {
  /** Set when the map could not initialise (e.g. no WebGL). */
  initError: string | null
  /** Apply a telemetry snapshot: marker, attitude, trail and projection. */
  setSnapshot(snapshot: TelemetrySnapshot | null): void
  /** Re-render the mission waypoint layer. */
  setMission(items: MissionItem[], selectedSeq: number | null): void
  /** Frame the home area and stop following. */
  goHome(): void
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

  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    let viewer: Cesium.Viewer
    try {
      viewer = createViewer(container)
    } catch (e) {
      setInitError(e instanceof Error ? e.message : String(e))
      return
    }

    const layer = createDroneLayer(viewer, () => trailPos.current, () => predictPos.current)
    droneRef.current = layer
    wpHandlerRef.current = installWaypointDrag(viewer)
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
      viewer.destroy()
      viewerRef.current = null
      droneRef.current = null
      followRef.current = null
      trailPos.current = []
      predictPos.current = []
    }
  }, [containerRef])

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

  const setMission = useCallback((items: MissionItem[], selectedSeq: number | null) => {
    const viewer = viewerRef.current
    if (!viewer) return
    disposeWaypointLayer(viewer, wpLayerRef.current)
    wpLayerRef.current = renderWaypoints(viewer, items, selectedSeq)
  }, [])

  const goHome = useCallback(() => {
    const viewer = viewerRef.current
    if (!viewer) return
    // Home is a wide overview, so stop following or the next frame re-centres.
    useUiStore.getState().setFollow(false)
    viewer.camera.lookAtTransform(Cesium.Matrix4.IDENTITY)
    viewer.camera.flyTo({ destination: Cesium.Cartesian3.fromDegrees(HOME_LON, HOME_LAT, 12000) })
  }, [])

  return { initError, setSnapshot, setMission, goHome }
}
