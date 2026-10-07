// Dead-reckon advance and the hand-rolled camera-follow loop. Extracted from
// `MapView.tsx` (issues.md #29).

import * as Cesium from 'cesium'
import { DEAD_RECKON_MAX_S, FOLLOW_PITCH_DEG, FOLLOW_RANGE_M } from './constants'
import { useUiStore } from '../stores/ui'

/** Latest fix plus its ENU velocity, used to advance the marker between updates. */
export interface VehicleFix {
  cart: Cesium.Cartesian3
  vel: Cesium.Cartesian3
  atMs: number
}

export interface FollowController {
  /** Record a new fix; snaps the marker to it immediately. */
  setFix(fix: VehicleFix): void
  /** Start advancing the marker on each pre-render frame. */
  attach(): void
  /** Remove the pre-render listener. */
  detach(): void
}

/**
 * Advance the marker past its last fix using the reported ENU velocity so the
 * icon and the followed camera glide between telemetry updates instead of
 * stepping at the update rate. When `follow` is on, re-centre the camera by
 * hand: Cesium's own `trackedEntity` derives its offset from the entity's
 * bounding sphere, which `model.minimumPixelSize` makes view-dependent, so the
 * camera oscillates and the whole map shakes.
 */
export function createFollowController(
  viewer: Cesium.Viewer,
  dronePosition: Cesium.ConstantPositionProperty,
): FollowController {
  let fix: VehicleFix | null = null
  let followCentred = false
  const advanced = new Cesium.Cartesian3()

  const followFix = () => {
    if (!fix) return
    const elapsed = Math.min((performance.now() - fix.atMs) / 1000, DEAD_RECKON_MAX_S)
    Cesium.Cartesian3.multiplyByScalar(fix.vel, Math.max(elapsed, 0), advanced)
    Cesium.Cartesian3.add(fix.cart, advanced, advanced)
    dronePosition.setValue(advanced)
    if (!useUiStore.getState().follow) {
      followCentred = false
      return
    }
    const camera = viewer.camera
    if (!followCentred) {
      followCentred = true
      camera.lookAt(
        advanced,
        new Cesium.HeadingPitchRange(0, Cesium.Math.toRadians(FOLLOW_PITCH_DEG), FOLLOW_RANGE_M),
      )
      return
    }
    const range = Cesium.Cartesian3.distance(camera.positionWC, advanced)
    camera.lookAt(advanced, new Cesium.HeadingPitchRange(camera.heading, camera.pitch, range))
  }

  return {
    setFix(next) {
      fix = next
      dronePosition.setValue(next.cart)
    },
    attach() {
      viewer.scene.preRender.addEventListener(followFix)
    },
    detach() {
      viewer.scene.preRender.removeEventListener(followFix)
      fix = null
    },
  }
}
