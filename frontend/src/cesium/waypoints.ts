// Map tools for the planning view: select/drag a waypoint, or add one by
// clicking the map (issues.md #29, findings 13/14/15/17/18).
//
// Select: a press on a waypoint previews on MOUSE_MOVE and commits to the store
// exactly once, on release, so one drag is one undo entry and one dirty
// transition. A press/release that never moves beyond the click threshold
// selects instead of moving.
//
// Add: a click (press/release <= threshold) picks the ground point and appends a
// waypoint; dragging pans the camera and adds nothing. Hovering shows a ghost
// point at the picked ground.
//
// Camera rotate/translate are disabled for the duration of a *select* drag and
// restored by a shared `finish()`, also wired to the window's
// `pointerup`/`blur` and `Escape`, so releasing outside the canvas can never
// leave the camera locked. Escape also returns to the select tool. The handler
// is inert outside the planning view.

import * as Cesium from 'cesium'
import type { GeoPoint } from '../generated-types/GeoPoint'
import { useMissionStore } from '../stores/mission'
import { usePolygonStore } from '../stores/polygon'
import { useUiStore } from '../stores/ui'
import type { MapTool } from '../stores/ui'
import { pickLatLon } from './pick'

/** Press/release displacement at or below this (px) counts as a click, not a drag. */
export const CLICK_DRAG_THRESHOLD_PX = 3

/** A ground point under the cursor, in degrees. */
export interface GroundPoint {
  lat: number
  lon: number
}

/** A click within this many pixels of the first polygon vertex closes it. */
const POLYGON_CLOSE_PX = 12

export interface MapToolOptions {
  /** Whether editing is allowed right now (the planning view). Default: always. */
  enabled?: () => boolean
  /** Active tool. Default: always `select`. */
  tool?: () => MapTool
  /** Move the on-screen point/line without touching the store (called per move). */
  onPreview?: (seq: number, lat: number, lon: number) => void
  /** A press/release inside the click threshold selects the waypoint. */
  onSelect?: (seq: number) => void
  /** A click with the add tool places a waypoint here. */
  onAdd?: (lat: number, lon: number) => void
  /** Ghost-point preview while hovering with the add tool; `null` in the sky. */
  onHover?: (ground: GroundPoint | null) => void
}

/**
 * Install the map tool handler on `viewer`. Returns the handler; call
 * `destroy()` on teardown (it also removes the window-level safety listeners).
 */
export function installMapTools(
  viewer: Cesium.Viewer,
  opts: MapToolOptions = {},
): Cesium.ScreenSpaceEventHandler {
  const handler = new Cesium.ScreenSpaceEventHandler(viewer.scene.canvas)
  const controller = viewer.scene.screenSpaceCameraController

  let pendingSeq = -1
  let dragging = false
  let pressing = false
  let start: Cesium.Cartesian2 | null = null
  let lastMouse: Cesium.Cartesian2 | null = null
  let lastGround: { lat: number; lon: number } | null = null
  /** Polygon vertex being dragged (`pg-<i>`), if the press landed on one. */
  let polygonDragSeq: number | null = null

  const enabled = () => (opts.enabled ? opts.enabled() : true)
  const tool = (): MapTool => (opts.tool ? opts.tool() : 'select')

  const restoreCamera = () => {
    controller.enableRotate = true
    controller.enableTranslate = true
  }

  const reset = () => {
    pendingSeq = -1
    dragging = false
    pressing = false
    start = null
    lastMouse = null
    lastGround = null
    polygonDragSeq = null
    restoreCamera()
    opts.onHover?.(null)
  }

  /** Commit a completed select gesture (idempotent across canvas + window). */
  const finish = () => {
    if (pendingSeq < 0) {
      pressing = false
      return
    }
    const seq = pendingSeq
    const moved = dragging
    const ground = lastGround
    reset()
    if (moved) {
      if (ground) {
        useMissionStore.getState().updatePosition(seq, ground.lat, ground.lon)
      }
    } else {
      opts.onSelect?.(seq)
    }
  }

  /** Commit a completed add click (idempotent across canvas + window). */
  const finishAdd = () => {
    if (!pressing) return
    const at = pressPosition()
    const moved =
      start && at ? Cesium.Cartesian2.distance(start, at) > CLICK_DRAG_THRESHOLD_PX : false
    pressing = false
    start = null
    if (moved || !at) return
    const ground = pickLatLon(viewer, at)
    if (ground) opts.onAdd?.(ground.lat, ground.lon)
  }

  const pressPosition = (): Cesium.Cartesian2 | null => lastMouse ?? start

  /** Whether `pos` is within [`POLYGON_CLOSE_PX`] of a polygon vertex on screen. */
  const nearVertex = (pos: Cesium.Cartesian2, v: GeoPoint): boolean => {
    const cart = Cesium.Cartesian3.fromDegrees(v.longitude_deg, v.latitude_deg, 0)
    const screen = Cesium.SceneTransforms.worldToWindowCoordinates(viewer.scene, cart)
    if (!screen) return false
    return Cesium.Cartesian2.distance(screen, pos) <= POLYGON_CLOSE_PX
  }

  /**
   * Commit a completed polygon gesture: a click adds a vertex, or closes the
   * boundary when it lands on the first vertex. Idempotent across canvas +
   * window (like `finishAdd`).
   */
  const finishPolygonClick = (at: Cesium.Cartesian2 | null) => {
    if (!pressing) return
    const moved =
      start && at ? Cesium.Cartesian2.distance(start, at) > CLICK_DRAG_THRESHOLD_PX : false
    pressing = false
    start = null
    polygonDragSeq = null
    restoreCamera()
    if (moved || !at) return
    const ground = pickLatLon(viewer, at)
    if (!ground) return
    const st = usePolygonStore.getState()
    if (st.closed) return
    const first = st.vertices[0]
    if (first && nearVertex(at, first)) st.close()
    else st.addVertex({ latitude_deg: ground.lat, longitude_deg: ground.lon })
  }

  handler.setInputAction(
    (click: Cesium.ScreenSpaceEventHandler.PositionedEvent) => {
      if (!enabled()) return
      start = Cesium.Cartesian2.clone(click.position)
      lastMouse = Cesium.Cartesian2.clone(click.position)
      const active = tool()
      if (active === 'add') {
        pressing = true
        return
      }
      if (active === 'polygon') {
        pressing = true
        const picked = viewer.scene.pick(click.position)
        const entity = Cesium.defined(picked) ? (picked.id as Cesium.Entity) : undefined
        const seq = Number.parseInt(entity?.id ? String(entity.id).replace(/^pg-/, '') : '', 10)
        if (!Number.isNaN(seq)) {
          // Dragging a polygon vertex moves it; pressing empty ground pans.
          polygonDragSeq = seq
          controller.enableRotate = false
          controller.enableTranslate = false
        } else {
          polygonDragSeq = null
        }
        return
      }
      const picked = viewer.scene.pick(click.position)
      const entity = Cesium.defined(picked) ? (picked.id as Cesium.Entity) : undefined
      const seq = Number.parseInt(entity?.id ? String(entity.id).replace(/^wp-/, '') : '', 10)
      if (Number.isNaN(seq)) return
      pendingSeq = seq
      dragging = false
      lastGround = null
      controller.enableRotate = false
      controller.enableTranslate = false
    },
    Cesium.ScreenSpaceEventType.LEFT_DOWN,
  )

  handler.setInputAction(
    (movement: Cesium.ScreenSpaceEventHandler.MotionEvent) => {
      lastMouse = Cesium.Cartesian2.clone(movement.endPosition)
      const active = tool()
      if (pressing && active === 'add') {
        // Moving past the threshold turns the click into a camera pan.
        if (start && Cesium.Cartesian2.distance(start, movement.endPosition) > CLICK_DRAG_THRESHOLD_PX) {
          pressing = false
        }
        return
      }
      if (active === 'polygon') {
        if (pressing && polygonDragSeq !== null && start) {
          // Dragging a vertex: past the threshold it starts moving.
          if (Cesium.Cartesian2.distance(start, movement.endPosition) <= CLICK_DRAG_THRESHOLD_PX) return
          const ground = pickLatLon(viewer, movement.endPosition)
          if (ground) {
            usePolygonStore.getState().moveVertex(polygonDragSeq, {
              latitude_deg: ground.lat,
              longitude_deg: ground.lon,
            })
          }
          return
        }
        if (!pressing) {
          const ground = pickLatLon(viewer, movement.endPosition)
          opts.onHover?.(ground ? { lat: ground.lat, lon: ground.lon } : null)
        }
        return
      }
      if (pendingSeq < 0) {
        if (active === 'add') {
          const ground = pickLatLon(viewer, movement.endPosition)
          opts.onHover?.(ground ? { lat: ground.lat, lon: ground.lon } : null)
        }
        return
      }
      if (!dragging && start) {
        if (Cesium.Cartesian2.distance(start, movement.endPosition) <= CLICK_DRAG_THRESHOLD_PX) {
          return
        }
        dragging = true
      }
      const ground = pickLatLon(viewer, movement.endPosition)
      if (!ground) return
      lastGround = { lat: ground.lat, lon: ground.lon }
      opts.onPreview?.(pendingSeq, ground.lat, ground.lon)
    },
    Cesium.ScreenSpaceEventType.MOUSE_MOVE,
  )

  handler.setInputAction((click: Cesium.ScreenSpaceEventHandler.PositionedEvent) => {
    if (tool() === 'polygon') {
      finishPolygonClick(click.position)
    } else if (pressing) finishAdd()
    else finish()
  }, Cesium.ScreenSpaceEventType.LEFT_UP)

  // A double-click closes the boundary at the current vertex.
  handler.setInputAction(() => {
    if (tool() !== 'polygon' || !enabled()) return
    const st = usePolygonStore.getState()
    if (!st.closed && st.vertices.length >= 3) st.close()
  }, Cesium.ScreenSpaceEventType.LEFT_DOUBLE_CLICK)

  // A gesture can end off-canvas (or the window can lose focus mid-drag); the
  // window listeners make sure the camera is always restored (finding 15).
  const onWindowPointerUp = () => {
    if (tool() === 'polygon') finishPolygonClick(lastMouse ?? start)
    else if (pressing) finishAdd()
    else finish()
  }
  const onWindowBlur = () => reset()
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== 'Escape') return
    reset()
    useUiStore.getState().setMapTool('select')
    if (usePolygonStore.getState().vertices.length > 0) usePolygonStore.getState().reset()
  }
  window.addEventListener('pointerup', onWindowPointerUp)
  window.addEventListener('blur', onWindowBlur)
  window.addEventListener('keydown', onKeyDown)

  const originalDestroy = handler.destroy.bind(handler)
  handler.destroy = () => {
    window.removeEventListener('pointerup', onWindowPointerUp)
    window.removeEventListener('blur', onWindowBlur)
    window.removeEventListener('keydown', onKeyDown)
    restoreCamera()
    originalDestroy()
  }

  return handler
}
