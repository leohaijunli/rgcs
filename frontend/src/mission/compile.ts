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

/**
 * Whether `command` is flown as a coordinate item, or as a `MAV_FRAME_MISSION`
 * command item whose arguments are the four command parameters.
 *
 * Mirrors `core::mission::command_uses_coordinate` (PX4's
 * `parse_mavlink_mission_item` split, which is also how QGroundControl writes
 * `.plan` files). A command outside this list must not carry a global frame:
 * PX4 rejects the whole upload with `MAV_MISSION_UNSUPPORTED` (issues.md #34).
 */
const COORDINATE_COMMANDS: readonly number[] = [
  16, // NAV_WAYPOINT
  17, // NAV_LOITER_UNLIM
  18, // NAV_LOITER_TURNS
  19, // NAV_LOITER_TIME
  21, // NAV_LAND
  22, // NAV_TAKEOFF
  31, // NAV_LOITER_TO_ALT
  82, // NAV_SPLINE_WAYPOINT
  84, // NAV_VTOL_TAKEOFF
  85, // NAV_VTOL_LAND
  80, // NAV_ROI
  113, // CONDITION_GATE
  179, // DO_SET_HOME
  195, // DO_SET_ROI_LOCATION
  201, // DO_SET_ROI
  400, // COMPONENT_ARM_DISARM
  4501, // NAV_FENCE_RETURN_POINT
  5001, // NAV_FENCE_POLYGON_VERTEX_INCLUSION
  5002, // NAV_FENCE_POLYGON_VERTEX_EXCLUSION
  5003, // NAV_FENCE_CIRCLE_INCLUSION
  5004, // NAV_FENCE_CIRCLE_EXCLUSION
  5100, // NAV_RALLY_POINT
]

export function commandUsesCoordinate(command: number): boolean {
  return COORDINATE_COMMANDS.includes(command)
}

/**
 * Wire frame each UI altitude mode compiles to.
 *
 * AGL deliberately does **not** use `MAV_FRAME_GLOBAL_TERRAIN_ALT_INT`: PX4
 * v1.17 rejects every terrain frame on upload (`MAV_MISSION_UNSUPPORTED_FRAME`,
 * the sibling of issues.md #34), and ADR-005 puts terrain following in the
 * ground station rather than relying on the FC's terrain database. Until a DEM
 * exists the ground is the flat plane through HOME — exactly what the relative
 * frame means — so AGL compiles to the relative frame and becomes a true AGL
 * profile once WS-D drapes the path (issues.md #40).
 */
export const FRAME_BY_MODE: Record<AltitudeMode, MissionFrame> = {
  relative: 'global_relative_alt_int',
  amsl: 'global_int',
  agl: 'global_relative_alt_int',
}

/**
 * Map a wire frame back to the UI altitude mode, if it has one.
 *
 * A frame with no altitude meaning (`local_*`, `mission`) returns null: those
 * items are not editable waypoints.
 */
export function modeFromFrame(frame: MissionFrame): AltitudeMode | null {
  switch (frame) {
    case 'global_int':
      return 'amsl'
    case 'global_relative_alt_int':
      return 'relative'
    case 'global_terrain_alt_int':
      // Only ever seen on a plan from another GCS: we compile AGL to the
      // relative frame because PX4 rejects terrain frames (issues.md #40).
      return 'agl'
    default:
      return null
  }
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
    case 'mission':
      // A command item: `z` is a command argument, not an altitude.
      return z
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
    case 'mission':
      return amslM
    default:
      return amslM
  }
}

/**
 * Narrow a value to the precision MAVLink actually carries.
 *
 * `MissionItem.z`/`params` are `f32` in `core::mission::MissionItem`, so the
 * value that reaches the FC is `fround(v)`. Keeping the local copy at f64 made
 * the post-upload read-back compare `80.123456789` with `80.12346` and report a
 * false "FC mission differs from the uploaded plan" (issues.md #38).
 */
function wire(v: number): number {
  return Math.fround(v)
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
 * matching `core::plan::PlannedMission::compile`. Command items (see
 * [`commandUsesCoordinate`]) keep their stored `z`: it is a command argument,
 * not an altitude, so it is never re-datumed.
 */
export function compileWaypoints(
  waypoints: PlannedWaypoint[],
  mode: AltitudeMode,
  homeAmslM: number,
): MissionItem[] {
  const frame = FRAME_BY_MODE[mode]
  return waypoints.map((wp, seq) => {
    const coordinate = commandUsesCoordinate(wp.command)
    const itemFrame = coordinate ? frame : ('mission' as MissionFrame)
    return {
      seq,
      frame: itemFrame,
      command: wp.command,
      params: Array.from(params4(wp.params), wire),
      x: Math.round(wp.position.latitude_deg * 1e7),
      y: Math.round(wp.position.longitude_deg * 1e7),
      z: wire(coordinate ? amslToFrame(wp.altitude.meters, frame, homeAmslM) : wp.altitude.meters),
      autocontinue: wp.autocontinue,
      current: seq === 0,
    }
  })
}

/**
 * Wire items with `z` expressed as AMSL, for drawing on the map.
 *
 * The map is drawn in a global frame, but the plan may be compiled to the
 * relative (or terrain) frame, where `z` is only an offset from HOME; drawing
 * those numbers straight away put the path at the wrong height. Command items
 * are left alone — they have no position (`mission/lineKinds.ts` skips them).
 */
export function toDisplayItems(items: MissionItem[], homeAmslM: number): MissionItem[] {
  return items.map((item) =>
    item.frame === 'global_int' || item.frame === 'mission'
      ? item
      : { ...item, frame: 'global_int', z: frameToAmsl(item.z, item.frame, homeAmslM) },
  )
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
