// Screen-space waypoint dragging committed to the mission store. Extracted from
// `MapView.tsx` (issues.md #29).

import * as Cesium from 'cesium'
import { useMissionStore } from '../stores/mission'

/**
 * Pick a waypoint by its `wp-<seq>` entity id on left-down, move it with the
 * cursor, and commit to the mission store on drag. Camera rotate/translate are
 * disabled for the duration of a drag so the gesture does not spin the map.
 * Returns the handler; call `destroy()` on teardown.
 */
export function installWaypointDrag(viewer: Cesium.Viewer): Cesium.ScreenSpaceEventHandler {
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
      const cartesian = viewer.camera.pickEllipsoid(movement.endPosition, viewer.scene.globe.ellipsoid)
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
  return handler
}
