import { useEffect, useRef, useState } from 'react'
import * as Cesium from 'cesium'
import 'cesium/Build/Cesium/Widgets/widgets.css'
import { cssVar } from '../design-system/theme'
import { degFromMavInt, useMissionStore } from '../stores/mission'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore } from '../stores/ui'
import MapToolbar from './MapToolbar'

const HOME_LAT = 48.6493
const HOME_LON = -123.3982
const TRAIL_MAX = 512
const TRAIL_EVERY = 2

export default function MapView() {
  const containerRef = useRef<HTMLDivElement>(null)
  const [initError, setInitError] = useState<string | null>(null)
  const viewerRef = useRef<Cesium.Viewer | null>(null)
  const droneRef = useRef<Cesium.Entity | null>(null)
  const trailRef = useRef<Cesium.Entity | null>(null)
  const homeRef = useRef<Cesium.Entity | null>(null)
  const trailPos = useRef<Cesium.Cartesian3[]>([])
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

    // Heading arrow sprite (offline, theme-tinted).
    const arrowCanvas = document.createElement('canvas')
    arrowCanvas.width = 64
    arrowCanvas.height = 64
    const ag = arrowCanvas.getContext('2d')
    if (ag) {
      ag.translate(32, 32)
      ag.fillStyle = cssVar('--mg-accent')
      ag.beginPath()
      ag.moveTo(0, -22)
      ag.lineTo(15, 18)
      ag.lineTo(-15, 18)
      ag.closePath()
      ag.fill()
    }
    const arrowUrl = arrowCanvas.toDataURL('image/png')

    const drone = viewer.entities.add({
      position: Cesium.Cartesian3.fromDegrees(HOME_LON, HOME_LAT, 100),
      billboard: {
        image: arrowUrl,
        rotation: 0,
        width: 36,
        height: 36,
        verticalOrigin: Cesium.VerticalOrigin.CENTER,
      },
      label: {
        text: 'UAV ≈MSL',
        font: '12px sans-serif',
        pixelOffset: new Cesium.Cartesian2(0, -18),
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
    droneRef.current = drone
    trailRef.current = trail
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
      homeRef.current = null
      trailPos.current = []
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

    const cart = Cesium.Cartesian3.fromDegrees(pos.longitude_deg, pos.latitude_deg, pos.altitude.meters)
    drone.position = new Cesium.ConstantPositionProperty(cart)
    if (drone.billboard && pos.heading_deg) {
      drone.billboard.rotation = new Cesium.ConstantProperty(
        Cesium.Math.toRadians(pos.heading_deg),
      )
    }

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