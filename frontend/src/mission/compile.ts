// Frontend mirror of `core::plan` (ADR-013).
//
// The store keeps planned waypoints in absolute AMSL (`Height`, datum
// `AMSL_EGM96`); the wire frame is chosen when compiling for upload, export or
// the map. `core::plan::PlannedMission::compile` is the authority for the Rust
// side; this module keeps the same semantics so the map/upload path stays
// synchronous. Keep the two in step when either changes.
//
// AGL is a frontend-only mode for now: it maps to the onboard terrain frame,
// which PX4 v1.17 may not honour (finding 2), and it needs a DEM to be exact
// (WS-D). Until then the ground is assumed flat at the home altitude.

import type { Height } from '../generated-types/Height'
import type { MissionFrame } from '../generated-types/MissionFrame'
import type { MissionItem } from '../generated-types/MissionItem'
import type { PlannedWaypoint } from '../generated-types/PlannedWaypoint'
import type { AltitudeMode } from '../stores/mission'

/** The working datum: AMSL as reported by PX4 (ADR-006). */
export const AMSL_EGM96 = 'AMSL_EGM96' as const

/** Default clearance above home for a new waypoint when none is selected. */
export const DEFAULT_ALT_AGL_M = 50

/** Wire frame each UI altitude mode compiles to. */
export const FRAME_BY_MODE: Record<AltitudeMode, MissionFrame> = {
  relative: 'global_relative_alt_int',
  amsl: 'global_int',
  agl: 'global_terrain_alt_int',
}

/** Wrap a metres-AMSL value as an absolute, datum-tagged height. */
export function amslHeight(meters: number): Height {
  return { datum: AMSL_EGM96, meters }
}

/** A `NAV_WAYPOINT` planned at an AMSL altitude. */
export function makeWaypoint(latDeg: number, lonDeg: number, amslM: number): PlannedWaypoint {
  return {
    position: { latitude_deg: latDeg, longitude_deg: lonDeg },
    altitude: amslHeight(amslM),
    command: 16, // MAV_CMD_NAV_WAYPOINT
    params: [0, 0, 0, 0],
    autocontinue: true,
  }
}

/** Re-express a wire `z` in `frame` as metres AMSL. */
export function frameToAmsl(z: number, frame: MissionFrame, homeAmslM: number): number {
  switch (frame) {
    case 'global_int':
      return z
    case 'global_relative_alt_int':
    case 'global_terrain_alt_int':
      // No DEM yet: the ground is assumed flat at the home altitude (WS-D).
      return z + homeAmslM
    default:
      // Local frames are not convertible without the home/local origin; treat
      // the value as AMSL rather than dropping it.
      return z
  }
}

/** Re-express an AMSL altitude as `z` in `frame`. */
export function amslToFrame(amslM: number, frame: MissionFrame, homeAmslM: number): number {
  switch (frame) {
    case 'global_int':
      return amslM
    case 'global_relative_alt_int':
    case 'global_terrain_alt_int':
      return amslM - homeAmslM
    default:
      return amslM
  }
}

/** Normalise a wire params array to the 4-tuple the model uses. */
function params4(params: readonly number[]): [number, number, number, number] {
  const out: [number, number, number, number] = [0, 0, 0, 0]
  for (let i = 0; i < 4; i += 1) out[i] = params[i] ?? 0
  return out
}

/**
 * Compile planned waypoints to wire items under `mode`.
 *
 * `seq` is the array index (the relative frame is computed from `homeAmslM`),
 * matching `core::plan::PlannedMission::compile`.
 */
export function compileWaypoints(
  waypoints: PlannedWaypoint[],
  mode: AltitudeMode,
  homeAmslM: number,
): MissionItem[] {
  const frame = FRAME_BY_MODE[mode]
  return waypoints.map((wp, seq) => ({
    seq,
    frame,
    command: wp.command,
    params: Array.from(params4(wp.params)),
    x: Math.round(wp.position.latitude_deg * 1e7),
    y: Math.round(wp.position.longitude_deg * 1e7),
    z: amslToFrame(wp.altitude.meters, frame, homeAmslM),
    autocontinue: wp.autocontinue,
    current: seq === 0,
  }))
}

/** Convert a wire item (FC download or QGC import) to a planned AMSL waypoint. */
export function waypointFromItem(item: MissionItem, homeAmslM: number): PlannedWaypoint {
  return {
    position: { latitude_deg: item.x / 1e7, longitude_deg: item.y / 1e7 },
    altitude: amslHeight(frameToAmsl(item.z, item.frame, homeAmslM)),
    command: item.command,
    params: params4(item.params),
    autocontinue: item.autocontinue,
  }
}
