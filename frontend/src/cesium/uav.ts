// Pure Cesium math for the UAV marker: attitude, ground speed and the
// constant-velocity forward projection. Kept out of `MapView.tsx` so the map
// component is about wiring, not vector math (issues.md #29).

import * as Cesium from 'cesium'
import type { GlobalPositionInt } from '../generated-types/GlobalPositionInt'

/** Ground speed (m/s) from the NED velocity vector. */
export function groundSpeedMps(pos: GlobalPositionInt): number {
  return Math.hypot(pos.velocity.x_m_s, pos.velocity.y_m_s)
}

/**
 * Attitude quaternion in the vehicle's local east-north-up frame. After
 * Cesium's default glTF axis correction (asset up = glTF +Y, forward = glTF
 * +Z) the model's nose lies along the local **+X** axis, so the body frame is
 * +X = nose, +Y = wing, +Z = up. MAVLink ATTITUDE is a NED rotation: roll is
 * about the forward/nose axis (+X), pitch about the lateral/wing axis (+Y),
 * and yaw about the up/down axis (+Z/−Z). An asset whose nose is glTF +Z
 * additionally needs `UAV_MODEL_NOSE_YAW_OFFSET_DEG` to re-align the heading.
 */
export function uavQuaternion(
  yawDeg: number,
  pitchDeg: number,
  rollDeg: number,
): Cesium.Quaternion {
  const yaw = Cesium.Quaternion.fromAxisAngle(
    Cesium.Cartesian3.UNIT_Z,
    Cesium.Math.toRadians(-yawDeg),
    new Cesium.Quaternion(),
  )
  const pitch = Cesium.Quaternion.fromAxisAngle(
    Cesium.Cartesian3.UNIT_Y,
    Cesium.Math.toRadians(pitchDeg),
    new Cesium.Quaternion(),
  )
  const roll = Cesium.Quaternion.fromAxisAngle(
    Cesium.Cartesian3.UNIT_X,
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
 * World-space orientation for the UAV model, ready for `Entity.orientation`.
 *
 * An entity's `orientation` is applied directly in the earth-fixed frame, not
 * in a local east-north-up frame: an identity quaternion leaves the model
 * aligned with the ECEF axes, so a level airframe at mid latitude would render
 * tipped over. Compose the local attitude with the position's ENU-to-ECEF
 * rotation so heading, pitch and roll stay true to the MAVLink attitude.
 */
export function uavOrientation(
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
 * Constant-velocity forward projection sampled from the live fix. Because it
 * always starts at the current position, the segment already flown is never
 * part of the result: each fix replaces the hint and only the track ahead
 * remains.
 */
export function projectAhead(
  pos: GlobalPositionInt,
  horizonS: number,
  stepS: number,
): Cesium.Cartesian3[] {
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
