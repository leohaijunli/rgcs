// Recorded flight track: distance-decimated and drawn in one solid accent
// colour. Replaces the single `CallbackProperty` polyline, which kept ~50 s of
// track (512 points, every 2nd of 20 Hz) and rebuilt the whole geometry every
// frame.
//
// The track is drawn as `Primitive` polylines split into fixed-size chunks:
// a finished chunk is built once and never touched again, only the short tail
// chunk is rebuilt (throttled), so cost per update stays flat however long the
// flight is.

import * as Cesium from 'cesium'
import { cssVar } from '../design-system/theme'
import {
  TRAIL_CHUNK_POINTS,
  TRAIL_MAX_POINTS,
  TRAIL_MIN_STEP_M,
  TRAIL_TAIL_REBUILD_MS,
  TRAIL_WIDTH_PX,
} from './constants'

interface TrailPoint {
  lon: number
  lat: number
  amsl: number
}

export interface TrailLayer {
  /** Record a fix (AMSL metres). Fixes closer than `TRAIL_MIN_STEP_M` to the
   * last kept one are dropped. */
  push(lonDeg: number, latDeg: number, amslM: number): void
  /** Geoid/mesh datum offset added to every rendered height. */
  setOffset(offsetM: number): void
  clear(): void
  dispose(): void
}

export function createTrailLayer(viewer: Cesium.Viewer): TrailLayer {
  const scene = viewer.scene
  const accent = Cesium.Color.fromCssColorString(cssVar('--mg-accent'))
  const stride = TRAIL_CHUNK_POINTS - 1 // chunks share their end point

  let points: TrailPoint[] = []
  let chunks: Cesium.Primitive[] = []
  let tail: Cesium.Primitive | null = null
  let offset = 0
  let lastCart: Cesium.Cartesian3 | null = null
  let dirty = false
  let lastTailBuild = 0

  const cartOf = (p: TrailPoint) => Cesium.Cartesian3.fromDegrees(p.lon, p.lat, p.amsl + offset)

  const build = (slice: TrailPoint[]): Cesium.Primitive | null => {
    if (slice.length < 2) return null
    return new Cesium.Primitive({
      geometryInstances: new Cesium.GeometryInstance({
        geometry: new Cesium.PolylineGeometry({
          positions: slice.map(cartOf),
          colors: slice.map(() => accent),
          colorsPerVertex: true,
          width: TRAIL_WIDTH_PX,
          vertexFormat: Cesium.PolylineColorAppearance.VERTEX_FORMAT,
        }),
      }),
      appearance: new Cesium.PolylineColorAppearance(),
      asynchronous: false,
    })
  }

  const dropTail = () => {
    if (tail) scene.primitives.remove(tail)
    tail = null
  }

  const rebuildTail = () => {
    dropTail()
    tail = build(points.slice(chunks.length * stride))
    if (tail) scene.primitives.add(tail)
    dirty = false
    lastTailBuild = performance.now()
  }

  /** Close every full chunk (each built once). */
  const closeChunks = () => {
    while (points.length - chunks.length * stride >= TRAIL_CHUNK_POINTS) {
      const start = chunks.length * stride
      const done = build(points.slice(start, start + TRAIL_CHUNK_POINTS))
      if (!done) break
      scene.primitives.add(done)
      chunks.push(done)
      dropTail()
    }
  }

  const removeChunks = (count: number) => {
    for (const c of chunks.splice(0, count)) scene.primitives.remove(c)
  }

  const rebuildAll = () => {
    removeChunks(chunks.length)
    closeChunks()
    rebuildTail()
    const last = points[points.length - 1]
    lastCart = last ? cartOf(last) : null
  }

  // Throttled tail refresh: at most one rebuild per `TRAIL_TAIL_REBUILD_MS`,
  // flushed on the render loop so the last fix always shows up.
  const flush = () => {
    if (dirty && performance.now() - lastTailBuild >= TRAIL_TAIL_REBUILD_MS) rebuildTail()
  }
  scene.preRender.addEventListener(flush)

  return {
    push(lon, lat, amsl) {
      if (![lon, lat, amsl].every(Number.isFinite)) return
      const point: TrailPoint = { lon, lat, amsl }
      const cart = cartOf(point)
      if (lastCart && Cesium.Cartesian3.distance(cart, lastCart) < TRAIL_MIN_STEP_M) return
      lastCart = cart
      points.push(point)
      closeChunks()
      dirty = true
      // Over the cap: drop the oldest whole chunks (keeps chunk alignment).
      if (points.length > TRAIL_MAX_POINTS && chunks.length > 0) {
        const drop = Math.min(chunks.length, Math.ceil((points.length - TRAIL_MAX_POINTS) / stride))
        removeChunks(drop)
        points = points.slice(drop * stride)
      }
    },
    setOffset(next) {
      if (next === offset) return
      offset = next
      rebuildAll()
    },
    clear() {
      removeChunks(chunks.length)
      dropTail()
      points = []
      lastCart = null
      dirty = false
    },
    dispose() {
      scene.preRender.removeEventListener(flush)
      removeChunks(chunks.length)
      dropTail()
      points = []
    },
  }
}
