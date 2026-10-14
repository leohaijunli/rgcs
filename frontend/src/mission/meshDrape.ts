// Terrain drape against the live 3D mesh (the frontend twin of
// `core::terrain::drape`).
//
// The 3D mesh is a Cesium tileset: the loopback terrain server serves it to the
// webview, so the backend `terrain_drape` (which needs a dem-prep folder it can
// read) cannot reach it. When the mesh is live it is the surface the operator
// actually sees, so terrain mode measures the survey AGL against it — the
// priority order is mesh → 2D DEM tile → nothing (no drape).
//
// Datum care (AGENTS height rules): a tileset built with `--geoid` serves WGS84
// ellipsoid heights (`h`), so AMSL = `h − N(EGM96)`; a tileset built from
// CGVD2013 orthometric data serves `H` directly (≈ AMSL, ≤ ~20 cm inside one
// region). Whatever the source, the stored waypoint stays **absolute AMSL**
// (ADR-013) — this module only rewrites `altitude.meters`.
//
// Pure and Cesium-free so `scripts/check-agl.mjs` can assert the drape math
// directly (same contract as `cesium/heights.ts`).

import type { PlannedWaypoint } from '../generated-types/PlannedWaypoint'

/** MAV_CMD value that carries a coordinate (`NAV_WAYPOINT`); command items
 * (`DO_*`) pass through — mirrors `core::terrain::drape`. */
const NAV_WAYPOINT = 16

/** Coordinate commands that must finish *on* the ground rather than `aglM`
 * above it: `NAV_LAND` / `NAV_VTOL_LAND` (operator report: a landing whose
 * target sat on the HOME plane left the vehicle hovering over lower ground). */
const LAND_COMMANDS: readonly number[] = [21, 85]

/** Drape options for the mesh path. The DSM clearance check has no mesh
 * equivalent (a tileset is one surface, no DTM/DSM pair): the report says so
 * instead of pretending a clearance was measured. */
export interface MeshDrapeOptions {
  /** Requested clearance above the mesh surface, metres. */
  aglM: number
  /** EGM96 undulation per waypoint **index**, subtracted from an
   * ellipsoid-datum mesh sample to reach AMSL. `null`/missing = the sample is
   * already AMSL (orthometric mesh) — no conversion. */
  geoidByIndex?: ReadonlyMap<number, number> | null
  /** How far above the ground a landing waypoint's target is placed, metres.
   * Zero aims at the surface itself. */
  landClearanceM?: number
  /** Draped heights must never fall below the first waypoint's draped height
   * minus this much (the return-climb floor), metres. `null` = no floor. */
  floorBelowStartM?: number | null
}

/** What a mesh drape did, for the Pattern step readout. */
export interface MeshDrapeResult {
  /** The plan with mesh-draped absolute AMSL heights. */
  waypoints: PlannedWaypoint[]
  /** Coordinate waypoints whose height was rewritten. */
  draped: number
  /** Landing waypoints placed on the ground (not `aglM` above it). */
  landed: number
  /** Coordinate waypoints with no mesh sample (outside the tileset): left
   * untouched — the operator must clip the survey to the coverage. */
  missing: number
  /** Lowest / highest ground (AMSL) under the draped waypoints, metres. */
  minGroundAmslM: number
  maxGroundAmslM: number
  /** The clearance the draped waypoints actually fly (AGL), metres. */
  minAglM: number
  /** Which surface the heights were measured against. */
  reference: 'mesh'
}

/**
 * Rewrite survey waypoint heights so the aircraft flies `aglM` above the mesh
 * surface. Mirrors `core::terrain::drape::drape_to_terrain`: command items and
 * locked waypoints pass through, and a waypoint with no mesh sample is left
 * as-is (counted in `missing`) rather than guessed.
 */
export function drapeToMesh(
  waypoints: readonly PlannedWaypoint[],
  groundByIndex: ReadonlyMap<number, number>,
  options: MeshDrapeOptions,
): MeshDrapeResult {
  const out: PlannedWaypoint[] = []
  let draped = 0
  let landed = 0
  let missing = 0
  let minGround = Number.POSITIVE_INFINITY
  let maxGround = Number.NEGATIVE_INFINITY
  let minAgl = Number.POSITIVE_INFINITY

  waypoints.forEach((wp, index) => {
    const landing = LAND_COMMANDS.includes(wp.command)
    if ((wp.command !== NAV_WAYPOINT && !landing) || wp.locked) {
      out.push(wp)
      return
    }
    const sample = groundByIndex.get(index)
    if (sample == null || !Number.isFinite(sample)) {
      missing += 1
      out.push(wp)
      return
    }
    const groundAmsl = sample - (options.geoidByIndex?.get(index) ?? 0)
    // A landing waypoint aims at the ground itself; `aglM` (and the DEM
    // path's DSM clearance) would hold it in the air instead.
    const clearance = landing ? (options.landClearanceM ?? 0) : options.aglM
    const height = groundAmsl + clearance
    out.push({ ...wp, altitude: { ...wp.altitude, meters: height } })
    draped += 1
    if (landing) landed += 1
    minGround = Math.min(minGround, groundAmsl)
    maxGround = Math.max(maxGround, groundAmsl)
    minAgl = Math.min(minAgl, height - groundAmsl)
  })

  // Return-climb floor: nothing draped may fall below the start height by more
  // than the configured slack (keeps a valley floor from dipping below takeoff
  // on the way back). Locked waypoints are floored too — same as the backend.
  const slack = options.floorBelowStartM
  const first = out[0]
  if (slack != null && first != null) {
    const floor = first.altitude.meters - slack
    for (const wp of out) {
      if (wp.command === NAV_WAYPOINT && wp.altitude.meters < floor) {
        wp.altitude = { ...wp.altitude, meters: floor }
      }
    }
  }

  return {
    waypoints: out,
    draped,
    landed,
    missing,
    minGroundAmslM: Number.isFinite(minGround) ? minGround : 0,
    maxGroundAmslM: Number.isFinite(maxGround) ? maxGround : 0,
    minAglM: Number.isFinite(minAgl) ? minAgl : 0,
    reference: 'mesh',
  }
}
