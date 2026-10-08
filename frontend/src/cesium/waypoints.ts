// Screen-space waypoint dragging for the planning view (issues.md #29,
// findings 14/15/17/18).
//
// A drag updates an on-screen preview only and commits to the mission store
// exactly once, on release, so one drag is one undo entry and one dirty
// transition. A press/release that never moves beyond the click threshold is a
// selection, not a move. Camera rotate/translate are disabled for the duration
// of the gesture and restored by a shared `finish()` that is also wired to the
// window's `pointerup`/`blur` and `Escape`, so releasing outside the canvas can
// never leave the camera locked. The handler is inert outside the planning view.

import * as Cesium from 'cesium'
import { useMissionStore } from '../stores/mission'
import { pickLatLon } from './pick'

/** Press/release displacement at or below this (px) counts as a click, not a drag. */
export const CLICK_DRAG_THRESHOLD_PX = 3

export interface WaypointDragOptions {
  /** Whether editing is allowed right now (the planning view). Default: always. */
  enabled?: () => boolean
  /** Move the on-screen point/line without touching the store (called per move). */
  onPreview?: (seq: number, lat: number, lon: number) => void
  /** A press/release inside the click threshold selects the waypoint. */
  onSelect?: (seq: number) => void
}

/**
 * Install the drag handler on `viewer`. Returns the handler; call `destroy()`
 * on teardown (it also removes the window-level safety listeners).
 */
export function installWaypointDrag(
  viewer: Cesium.Viewer,
  opts: WaypointDragOptions = {},
): Cesium.ScreenSpaceEventHandler {
  const handler = new Cesium.ScreenSpaceEventHandler(viewer.scene.canvas)
  const controller = viewer.scene.screenSpaceCameraController

  let pendingSeq = -1
  let dragging = false
  let start: Cesium.Cartesian2 | null = null
  let lastGround: { lat: number; lon: number } | null = null

  const enabled = () => (opts.enabled ? opts.enabled() : true)

  const restoreCamera = () => {
    controller.enableRotate = true
    controller.enableTranslate = true
  }

  const reset = () => {
    pendingSeq = -1
    dragging = false
    start = null
    lastGround = null
    restoreCamera()
  }

  /** Commit a completed gesture (idempotent: safe to call from canvas + window). */
  const finish = () => {
    if (pendingSeq < 0) return
    const seq = pendingSeq
    const moved = dragging
    const ground = lastGround
    reset()
    if (moved) {
      if (ground) {
        useMissionStore.getState().updateItem(seq, {
          x: Math.round(ground.lat * 1e7),
          y: Math.round(ground.lon * 1e7),
        })
      }
    } else {
      opts.onSelect?.(seq)
    }
  }

  handler.setInputAction(
    (click: Cesium.ScreenSpaceEventHandler.PositionedEvent) => {
      if (!enabled()) return
      const picked = viewer.scene.pick(click.position)
      const entity = Cesium.defined(picked) ? (picked.id as Cesium.Entity) : undefined
      const seq = Number.parseInt(entity?.id ? String(entity.id).replace(/^wp-/, '') : '', 10)
      if (Number.isNaN(seq)) return
      pendingSeq = seq
      dragging = false
      start = Cesium.Cartesian2.clone(click.position)
      lastGround = null
      controller.enableRotate = false
      controller.enableTranslate = false
    },
    Cesium.ScreenSpaceEventType.LEFT_DOWN,
  )

  handler.setInputAction(
    (movement: Cesium.ScreenSpaceEventHandler.MotionEvent) => {
      if (pendingSeq < 0) return
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

  handler.setInputAction(() => finish(), Cesium.ScreenSpaceEventType.LEFT_UP)

  // A gesture can end off-canvas (or the window can lose focus mid-drag); the
  // window listeners make sure the camera is always restored (finding 15).
  const onWindowPointerUp = () => finish()
  const onWindowBlur = () => reset()
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Escape') reset()
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
