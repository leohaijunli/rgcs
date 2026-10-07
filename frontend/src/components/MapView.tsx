import { useEffect, useRef, useState } from 'react'
import * as Cesium from 'cesium'
import 'cesium/Build/Cesium/Widgets/widgets.css'
import { cssVar } from '../design-system/theme'
import type { GlobalPositionInt } from '../generated-types/GlobalPositionInt'
import i18n from '../i18n'
import { degFromMavInt, useMissionStore } from '../stores/mission'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore } from '../stores/ui'
import MapToolbar from './MapToolbar'

const HOME_LAT = 48.6493
const HOME_LON = -123.3982
const TRAIL_MAX = 512
const TRAIL_EVERY = 2

// Forward projection drawn ahead of the vehicle. Constant-velocity estimate
// from the live NED velocity; it is rebuilt from the current fix on every
// update, so any segment the aircraft has already flown is dropped and only
// the future track remains.
const PREDICT_HORIZON_S = 120
const PREDICT_STEP_S = 4
const PREDICT_MIN_GROUNDSPEED_M_S = 1

// Shipped airframe (see `model/README.md` at the repo root). Cesium maps the
// asset's glTF axes onto the body frame as +X -> north (+Y), +Y -> up,
// +Z -> east (+X), so the nose (the camera gimbal, glTF +Z) lies on the body's
// +X axis, 90 degrees clockwise of the +Y axis `uavQuaternion` calls the nose.
// Passing `yaw - 90` puts the nose back on the reported heading.
const UAV_MODEL_URI = '/model/scene-static.gltf'
const UAV_MODEL_NOSE_YAW_OFFSET_DEG = -90

export default function MapView() {
  const containerRef = useRef<HTMLDivElement>(null)
  const [initError, setInitError] = useState<string | null>(null)
  const viewerRef = useRef<Cesium.Viewer | null>(null)
  const droneRef = useRef<Cesium.Entity | null>(null)
  const trailRef = useRef<Cesium.Entity | null>(null)
  const predictRef = useRef<Cesium.Entity | null>(null)
  const homeRef = useRef<Cesium.Entity | null>(null)
  const trailPos = useRef<Cesium.Cartesian3[]>([])
  const predictPos = useRef<Cesium.Cartesian3[]>([])
  const tickRef = useRef(0)
  const wpLineRef = useRef<Cesium.Entity | null>(null)
  const wpPointRefs = useRef<Map<number, Cesium.Entity>>(new Map())
  const wpHandlerRef = useRef<Cesium.ScreenSpaceEventHandler | null>(null)

  const follow = useUiStore((s) => s.follow)
  const map3d = useUiStore((s) => s.map3d)

  // Viewer lifecycle.
  useEffect(() => {
    const container = containerRef.current
    if (!container) return
    try {
      // Fail loudly instead of a black map if the webview has no WebGL.
      const probe = document.createElement('canvas')
      if (!probe.getContext('webgl2') && !probe.getContext('webgl')) {
        throw new Error('WebGL is unavailable in this webview; the map cannot render.')
      }

    const grid = new Cesium.GridImageryProvider({
      cells: 16,
      color: Cesium.Color.fromCssColorString(cssVar('--mg-grid-line')),
      glowColor: Cesium.Color.fromCssColorString(cssVar('--mg-grid-glow')),
      backgroundColor: Cesium.Color.fromCssColorString(cssVar('--mg-grid-bg')),
      canvasSize: 256,
    })
    // OSM imagery (no Ion token) on top of the offline grid: real map when
    // online, grid fallback when offline.
    const osm = new Cesium.OpenStreetMapImageryProvider({ url: 'https://tile.openstreetmap.org/' })
    const viewer = new Cesium.Viewer(container, {
      baseLayer: new Cesium.ImageryLayer(grid),
      animation: false,
      timeline: false,
      fullscreenButton: false,
      homeButton: false,
      sceneModePicker: false,
      baseLayerPicker: false,
      geocoder: false,
      navigationHelpButton: false,
      infoBox: false,
      selectionIndicator: false,
    })
    viewer.imageryLayers.addImageryProvider(osm, 1)
    viewer.camera.setView({
      destination: Cesium.Cartesian3.fromDegrees(HOME_LON, HOME_LAT, 12000),
    })

    const accent = Cesium.Color.fromCssColorString(cssVar('--mg-accent'))
    const ok = Cesium.Color.fromCssColorString(cssVar('--mg-ok'))
    const warn = Cesium.Color.fromCssColorString(cssVar('--mg-warn'))
    // 3D UAV marker (runtime-generated glTF). A model is used instead of
    // entity boxes because Cesium re-evaluates a model's position/orientation
    // every frame, so it tracks the vehicle instead of freezing on first draw,
    // and minimumPixelSize keeps it readable at any zoom. Hidden until the
    // first fix: the map has no terrain provider, so an untagged placeholder
    // would sit in the air above the ellipsoid ground.
    const drone = viewer.entities.add({
      show: false,
      position: Cesium.Cartesian3.fromDegrees(HOME_LON, HOME_LAT, 0),
      orientation: new Cesium.ConstantProperty(Cesium.Quaternion.IDENTITY),
      model: {
        uri: UAV_MODEL_URI,
        minimumPixelSize: 44,
        maximumScale: 10,
        // Keep the airframe's own materials; the silhouette is what keeps it
        // legible against the imagery at any zoom.
        silhouetteColor: accent,
        silhouetteSize: 2,
      },
      label: {
        text: i18n.t('map.uav'),
        font: '12px sans-serif',
        pixelOffset: new Cesium.Cartesian2(0, -34),
        fillColor: Cesium.Color.WHITE,
      },
    })
    const trail = viewer.entities.add({
      polyline: {
        positions: new Cesium.CallbackProperty(() => trailPos.current, false),
        width: 2,
        material: new Cesium.PolylineGlowMaterialProperty({
          glowPower: 0.18,
          color: accent,
        }),
      },
    })
    // Forward-projection hint: dashed and warn-tinted so it reads as a
    // prediction, not the recorded track.
    const predict = viewer.entities.add({
      polyline: {
        positions: new Cesium.CallbackProperty(() => predictPos.current, false),
        width: 2,
        material: new Cesium.PolylineDashMaterialProperty({
          color: warn,
          dashLength: 16,
        }),
      },
    })
    const home = viewer.entities.add({
      position: Cesium.Cartesian3.fromDegrees(HOME_LON, HOME_LAT, 0),
      ellipse: {
        semiMajorAxis: 12,
        semiMinorAxis: 12,
        material: ok.withAlpha(0.35),
        outline: true,
        outlineColor: ok,
      },
      label: {
        text: 'HOME',
        font: '11px sans-serif',
        pixelOffset: new Cesium.Cartesian2(0, -16),
        fillColor: Cesium.Color.WHITE,
      },
    })

    // Waypoint dragging: pick by entity id `wp-<seq>`, commit to the mission store.
    const handler = new Cesium.ScreenSpaceEventHandler(viewer.scene.canvas)
    let draggingSeq = -1
    handler.setInputAction(
      (click: Cesium.ScreenSpaceEventHandler.PositionedEvent) => {
        const picked = viewer.scene.pick(click.position)
        const id = Cesium.defined(picked) ? (picked.id as Cesium.Entity) : undefined
        const seqStr = id?.id ? String(id.id).replace(/^wp-/, '') : ''
        const seq = Number.parseInt(seqStr, 10)
        if (Number.isNaN(seq)) return
        draggingSeq = seq
        viewer.scene.screenSpaceCameraController.enableRotate = false
        viewer.scene.screenSpaceCameraController.enableTranslate = false
      },
      Cesium.ScreenSpaceEventType.LEFT_DOWN,
    )
    handler.setInputAction(
      (movement: Cesium.ScreenSpaceEventHandler.MotionEvent) => {
        if (draggingSeq < 0) return
        const cartesian = viewer.camera.pickEllipsoid(
          movement.endPosition,
          viewer.scene.globe.ellipsoid,
        )
        if (!cartesian) return
        const carto = Cesium.Cartographic.fromCartesian(cartesian)
        useMissionStore.getState().updateItem(draggingSeq, {
          x: Math.round(Cesium.Math.toDegrees(carto.latitude) * 1e7),
          y: Math.round(Cesium.Math.toDegrees(carto.longitude) * 1e7),
        })
      },
      Cesium.ScreenSpaceEventType.MOUSE_MOVE,
    )
    handler.setInputAction(
      () => {
        draggingSeq = -1
        viewer.scene.screenSpaceCameraController.enableRotate = true
        viewer.scene.screenSpaceCameraController.enableTranslate = true
      },
      Cesium.ScreenSpaceEventType.LEFT_UP,
    )
    wpHandlerRef.current = handler

    viewerRef.current = viewer
    // Dev-only handle for inspecting the scene (e.g. via the DevTools protocol).
    if (import.meta.env.DEV) {
      ;(window as unknown as { __mgViewer?: Cesium.Viewer }).__mgViewer = viewer
    }
    droneRef.current = drone
    trailRef.current = trail
    predictRef.current = predict
    homeRef.current = home

    return () => {
      wpHandlerRef.current?.destroy()
      wpHandlerRef.current = null
      wpPointRefs.current.forEach((e) => viewer.entities.remove(e))
      wpPointRefs.current.clear()
      if (wpLineRef.current) viewer.entities.remove(wpLineRef.current)
      wpLineRef.current = null
      viewer.destroy()
      viewerRef.current = null
      droneRef.current = null
      trailRef.current = null
      predictRef.current = null
      homeRef.current = null
      trailPos.current = []
      predictPos.current = []
    }
    } catch (e) {
      setInitError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  // Follow mode / 2D-3D mode.
  useEffect(() => {
    const viewer = viewerRef.current
    if (!viewer) return
    if (map3d) viewer.scene.morphTo3D(0)
    else viewer.scene.morphTo2D(0)
  }, [map3d])

  // Telemetry updates: drone position, trail, camera follow.
  const snapshot = useTelemetryStore((s) => s.snapshot)

  // Follow: Cesium's built-in smooth tracking. Re-evaluates when telemetry
  // arrives so the camera follows the real GPS position, not the home default.
  const hasPos = useTelemetryStore((s) => s.snapshot?.global_position != null)
  useEffect(() => {
    const viewer = viewerRef.current
    if (!viewer) return
    if (follow && hasPos && droneRef.current) {
      viewer.trackedEntity = droneRef.current
    } else {
      viewer.trackedEntity = undefined
    }
  }, [follow, hasPos])

  useEffect(() => {
    const viewer = viewerRef.current
    const drone = droneRef.current
    const pos = snapshot?.global_position
    if (!viewer || !drone || !pos) return

    // Render height above the home ground, not AMSL: the map has no terrain
    // provider, so the home marker sits on the ellipsoid. Using relative_alt
    // keeps a landed vehicle on the ground instead of floating by the site's
    // MSL elevation. Switch back to MSL once a DEM/terrain provider lands.
    const cart = Cesium.Cartesian3.fromDegrees(pos.longitude_deg, pos.latitude_deg, pos.relative_alt_m)

    // Attitude straight from the MAVLink ATTITUDE message; fall back to the
    // reported heading until the first attitude sample arrives. The marker is a
    // world-space object, so orbiting the camera only changes the viewpoint.
    const attitude = snapshot?.attitude
    drone.position = new Cesium.ConstantPositionProperty(cart)
    drone.orientation = new Cesium.ConstantProperty(
      uavOrientation(
        cart,
        (attitude?.yaw_deg ?? pos.heading_deg) + UAV_MODEL_NOSE_YAW_OFFSET_DEG,
        attitude?.pitch_deg ?? 0,
        attitude?.roll_deg ?? 0,
      ),
    )
    drone.show = true

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
  }, [snapshot])

  const goHome = () => {
    const viewer = viewerRef.current
    if (!viewer) return
    viewer.trackedEntity = undefined
    viewer.camera.flyTo({
      destination: Cesium.Cartesian3.fromDegrees(HOME_LON, HOME_LAT, 12000),
    })
  }

  // Mission waypoints: render a polyline + numbered points, re-run when items change.
  const missionItems = useMissionStore((s) => s.items)
  const selectedSeq = useMissionStore((s) => s.selectedSeq)

  useEffect(() => {
    const viewer = viewerRef.current
    if (!viewer) return

    // Remove previous waypoint entities (points + line).
    wpPointRefs.current.forEach((e) => viewer.entities.remove(e))
    wpPointRefs.current.clear()
    if (wpLineRef.current) {
      viewer.entities.remove(wpLineRef.current)
      wpLineRef.current = null
    }
    if (missionItems.length === 0) return

    const accent = Cesium.Color.fromCssColorString(cssVar('--mg-accent'))
    const ok = Cesium.Color.fromCssColorString(cssVar('--mg-ok'))

    const line = viewer.entities.add({
      polyline: {
        positions: missionItems.map((it) =>
          Cesium.Cartesian3.fromDegrees(degFromMavInt(it.y), degFromMavInt(it.x), it.z + 5),
        ),
        width: 2,
        material: new Cesium.PolylineGlowMaterialProperty({
          glowPower: 0.18,
          color: accent.withAlpha(0.8),
        }),
      },
    })
    wpLineRef.current = line

    const accentColor = accent
    missionItems.forEach((it) => {
      const isSel = it.seq === selectedSeq
      const color = isSel ? accentColor : ok
      const entity = viewer.entities.add({
        id: `wp-${it.seq}`,
        position: Cesium.Cartesian3.fromDegrees(degFromMavInt(it.y), degFromMavInt(it.x), it.z + 5),
        point: {
          pixelSize: isSel ? 14 : 11,
          color,
          outlineColor: Cesium.Color.WHITE,
          outlineWidth: 2,
        },
        label: {
          text: String(it.seq),
          font: '11px sans-serif',
          pixelOffset: new Cesium.Cartesian2(0, -16),
          fillColor: Cesium.Color.WHITE,
        },
      })
      wpPointRefs.current.set(it.seq, entity)
    })
  }, [missionItems, selectedSeq])

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
      <MapToolbar onGoHome={goHome} onToggleMeasure={() => undefined} />
    </div>
  )
}

/** Ground speed (m/s) from the NED velocity vector. */
function groundSpeedMps(pos: GlobalPositionInt): number {
  return Math.hypot(pos.velocity.x_m_s, pos.velocity.y_m_s)
}

/**
 * World-space orientation for the UAV model, ready for `Entity.orientation`.
 *
 * An entity's `orientation` is applied directly in the earth-fixed frame, not
 * in a local east-north-up frame: an identity quaternion leaves the model
 * aligned with the ECEF axes, so a level airframe at mid latitude would render
 * tipped over. Compose the local attitude with the position's ENU-to-ECEF
 * rotation so heading, pitch and roll stay true to the MAVLink attitude.
 */
function uavOrientation(
  position: Cesium.Cartesian3,
  yawDeg: number,
  pitchDeg: number,
  rollDeg: number,
): Cesium.Quaternion {
  const frame = Cesium.Transforms.eastNorthUpToFixedFrame(position)
  const enuToEcef = Cesium.Quaternion.fromRotationMatrix(
    Cesium.Matrix4.getMatrix3(frame, new Cesium.Matrix3()),
    new Cesium.Quaternion(),
  )
  return Cesium.Quaternion.multiply(
    enuToEcef,
    uavQuaternion(yawDeg, pitchDeg, rollDeg),
    new Cesium.Quaternion(),
  )
}

/**
 * Attitude quaternion in the vehicle's local east-north-up frame. The body
 * frame is +X = right wing, +Y = nose, +Z = up, so a compass yaw rotates about
 * -Z, nose-up pitch about +X, and right-wing-down roll about +Y. Cesium maps
 * the asset's glTF +X here, so an asset whose nose is glTF +Z needs
 * [`UAV_MODEL_NOSE_YAW_OFFSET_DEG`] to re-align the heading.
 */
function uavQuaternion(yawDeg: number, pitchDeg: number, rollDeg: number): Cesium.Quaternion {
  const yaw = Cesium.Quaternion.fromAxisAngle(
    Cesium.Cartesian3.UNIT_Z,
    Cesium.Math.toRadians(-yawDeg),
    new Cesium.Quaternion(),
  )
  const pitch = Cesium.Quaternion.fromAxisAngle(
    Cesium.Cartesian3.UNIT_X,
    Cesium.Math.toRadians(pitchDeg),
    new Cesium.Quaternion(),
  )
  const roll = Cesium.Quaternion.fromAxisAngle(
    Cesium.Cartesian3.UNIT_Y,
    Cesium.Math.toRadians(rollDeg),
    new Cesium.Quaternion(),
  )
  return Cesium.Quaternion.multiply(
    Cesium.Quaternion.multiply(yaw, pitch, new Cesium.Quaternion()),
    roll,
    new Cesium.Quaternion(),
  )
}

/**
 * Constant-velocity forward projection sampled from the live fix. Because it
 * always starts at the current position, the segment already flown is never
 * part of the result: each fix replaces the hint and only the track ahead
 * remains.
 */
function projectAhead(pos: GlobalPositionInt, horizonS: number, stepS: number): Cesium.Cartesian3[] {
  const mPerDegLat = 111_320
  const cosLat = Math.max(Math.cos(Cesium.Math.toRadians(pos.latitude_deg)), 1e-6)
  const mPerDegLon = mPerDegLat * cosLat
  const north = pos.velocity.x_m_s
  const east = pos.velocity.y_m_s
  const climb = -pos.velocity.z_m_s
  const points: Cesium.Cartesian3[] = []
  for (let t = 0; t <= horizonS; t += stepS) {
    points.push(
      Cesium.Cartesian3.fromDegrees(
        pos.longitude_deg + (east * t) / mPerDegLon,
        pos.latitude_deg + (north * t) / mPerDegLat,
        pos.relative_alt_m + climb * t,
      ),
    )
  }
  return points
}
