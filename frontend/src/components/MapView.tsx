import { useEffect, useRef, useState } from 'react'
import * as Cesium from 'cesium'
import 'cesium/Build/Cesium/Widgets/widgets.css'
import { cssVar } from '../design-system/theme'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore } from '../stores/ui'
import MapToolbar from './MapToolbar'

const HOME_LAT = 49.25
const HOME_LON = -123.1
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

  const follow = useUiStore((s) => s.follow)
  const map3d = useUiStore((s) => s.map3d)

  // Viewer lifecycle.
  useEffect(() => {
    const container = containerRef.current
    if (!container) return
    try {

    const canvas = document.createElement('canvas')
    canvas.width = 1024
    canvas.height = 512
    const g = canvas.getContext('2d')
    if (g) {
      g.fillStyle = cssVar('--mg-grid-bg')
      g.fillRect(0, 0, 1024, 512)
      g.strokeStyle = cssVar('--mg-grid-line')
      g.lineWidth = 1
      for (let x = 0; x <= 1024; x += 64) {
        g.beginPath()
        g.moveTo(x, 0)
        g.lineTo(x, 512)
        g.stroke()
      }
      for (let y = 0; y <= 512; y += 64) {
        g.beginPath()
        g.moveTo(0, y)
        g.lineTo(1024, y)
        g.stroke()
      }
    }
    const grid = new Cesium.SingleTileImageryProvider({
      url: canvas.toDataURL('image/png'),
      tileWidth: 1024,
      tileHeight: 512,
    })
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

    viewerRef.current = viewer
    droneRef.current = drone
    trailRef.current = trail
    homeRef.current = home

    return () => {
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

  // Follow: Cesium's built-in smooth tracking.
  useEffect(() => {
    const viewer = viewerRef.current
    if (!viewer) return
    if (follow && droneRef.current) {
      viewer.trackedEntity = droneRef.current
    } else {
      viewer.trackedEntity = undefined
    }
  }, [follow])

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