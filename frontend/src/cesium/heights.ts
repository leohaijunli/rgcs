// AGL ground reference for the vehicle's drop line / height readout, and
// every other "m AGL" the app derives from a live or plan height.
//
// Two operator bugs shaped this: (1) the map readout under the drone showed
// the AMSL altitude labelled "AGL" whenever the plan carried no HOME
// elevation — the reference fell back to 0 (the ellipsoid), so `AMSL − 0`
// echoed the absolute altitude; (2) a fixed HOME plane meant the readout did
// not change when the vehicle flew over lower ground. The reference is now
// the **DEM/mesh surface under the vehicle** when one is loaded (the priority
// source), then the plan/vehicle HOME plane, then the ellipsoid.
//
// Pure and Cesium-free so `scripts/check-agl.mjs` can assert it directly.

/** The three inputs that decide which ground the vehicle is measured against. */
export interface GroundReferenceInput {
  /** DEM / 3D-mesh ground under the point (AMSL), or `null` outside coverage.
   * Highest priority: this is the terrain the vehicle is actually over. */
  demGroundAmslM?: number | null
  /** Plan HOME elevation in AMSL (0 = the plan carries none). */
  planHomeAmslM: number
  /** PX4-reported AMSL altitude (`GLOBAL_POSITION_INT.alt`). */
  reportedAmslM: number
  /** PX4 height above home (`GLOBAL_POSITION_INT.relative_alt`). */
  relativeAltM: number
}

/**
 * Elevation (AMSL) of the ground the vehicle's AGL readout and drop line stand
 * on. Priority: the DEM/mesh surface under the point, then the plan's HOME
 * elevation, then PX4's own HOME (`AMSL − relative_alt`, where the vehicle
 * armed). The last two keep a landed vehicle reading 0 AGL instead of echoing
 * its AMSL altitude; only when nothing else is known does it fall back to 0
 * (the ellipsoid).
 */
export function groundReferenceAmsl(input: GroundReferenceInput): number {
  const { demGroundAmslM, planHomeAmslM, reportedAmslM, relativeAltM } = input
  if (demGroundAmslM != null && Number.isFinite(demGroundAmslM)) return demGroundAmslM
  if (planHomeAmslM !== 0) return planHomeAmslM
  const vehicleHomeAmsl = reportedAmslM - relativeAltM
  return Number.isFinite(vehicleHomeAmsl) ? vehicleHomeAmsl : 0
}

/** The drop line's height above ground — the value it is labelled with ("AGL"). */
export function heightAboveGroundM(input: GroundReferenceInput): number {
  return input.reportedAmslM - groundReferenceAmsl(input)
}

/**
 * "On the ground" band (m): an AGL within ± this of zero counts as standing on
 * the surface, not as being below it. The reference is never exact — DEM/DTM
 * vertical error against PX4's AMSL, or `relative_alt` estimator noise while
 * disarmed — so a landed vehicle reads a few decimetres either side of zero.
 * Shared by the drop line (which is hidden inside this band) and the recorded
 * track's colour (which must NOT paint those points as "below ground").
 */
export const AGL_GROUND_BAND_M = 1.5

/** True when an AGL (m) is genuinely below the ground reference — beyond the
 *  [`AGL_GROUND_BAND_M`] tolerance — rather than merely noise around zero.
 *  Pure so `scripts/check-agl.mjs` can assert it directly. */
export function isBelowGround(aglM: number): boolean {
  return Number.isFinite(aglM) && aglM < -AGL_GROUND_BAND_M
}

/** Inputs that decide which AMSL frame the live vehicle is placed in. */
export interface VehicleAmslAnchorInput {
  /** Landed self-calibration: `DEM ground − reported AMSL` captured while the
   * vehicle reported ON_GROUND (2026-10-18 field fix — PX4 does not always
   * stream HOME_POSITION, so this path needs no HOME fix at all). Preferred
   * when present. */
  anchorOffsetM?: number | null
  /** DEM / 3D-mesh ground under the FC HOME — the point the vehicle armed on
   * (`null` when unknown / outside coverage). The fallback anchor. */
  demGroundAtHomeM?: number | null
  /** PX4-reported AMSL altitude (`GLOBAL_POSITION_INT.alt`). */
  reportedAmslM: number
  /** PX4 height above home (`GLOBAL_POSITION_INT.relative_alt`). */
  relativeAltM: number
}

/**
 * The vehicle's altitude in the app's AMSL frame — the DEM datum the plan and
 * the 3D mesh are drawn in.
 *
 * PX4 anchors `alt` at its EKF origin, whose altitude is only as good as the
 * GNSS MSL fix at arming time; on a BC site with ~18 m DEM ground a vehicle
 * has measured ~3 m at takeoff (AGL −14 m, mission waypoints ~20 m above the
 * vehicle — operator report 2026-10-18). The anchor error is a **constant
 * bias**: `reported` tracks height *changes* exactly, so
 * `reported + offset` (offset captured while landed) is correct everywhere —
 * including landing at a different elevation than takeoff, where the DEM
 * under the current position cancels the bias and the AGL reads 0 again.
 * The fallback anchors on `relative_alt` + the DEM ground under HOME.
 */
export function vehicleAmslAnchoredM(input: VehicleAmslAnchorInput): number {
  const { anchorOffsetM, demGroundAtHomeM, reportedAmslM, relativeAltM } = input
  if (anchorOffsetM != null && Number.isFinite(anchorOffsetM)) {
    return reportedAmslM + anchorOffsetM
  }
  if (
    demGroundAtHomeM != null &&
    Number.isFinite(demGroundAtHomeM) &&
    Number.isFinite(relativeAltM)
  ) {
    return demGroundAtHomeM + relativeAltM
  }
  return reportedAmslM
}
