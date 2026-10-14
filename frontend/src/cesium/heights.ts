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
