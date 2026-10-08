// Altitude-datum conversion for the planning view (finding 1).
//
// A `MissionItem.z` is meaningless without its frame: switching the altitude
// mode used to leave existing items on the old frame, silently changing what
// the same number means. This module converts `z` between the three datums the
// plan supports so a mode switch preserves the physical height.
//
//	relative  z = AMSL - home AMSL
//	amsl      z = AMSL
//	agl       z = AMSL - ground AMSL
//
// The home/ground AMSL anchor comes from the imported plan's
// `plannedHomePosition` (QGC stores it in AMSL). There is no DEM yet (WS-D), so
// the ground under a waypoint is assumed to be at the home altitude: exact for
// a flat site, and the only assumption available before terrain data lands. The
// UI states the anchor so the conversion is never silent.

import type { AltitudeMode } from '../stores/mission'

/** Datum anchor for the conversion, in metres AMSL. */
export interface AltitudeAnchor {
  /** AMSL of the home point; also the assumed flat-ground elevation. */
  homeAmslM: number
}

export const DEFAULT_ALTITUDE_ANCHOR: AltitudeAnchor = { homeAmslM: 0 }

/** Convert a `z` expressed in `datum` to metres AMSL. */
export function zToAmsl(z: number, datum: AltitudeMode, anchor: AltitudeAnchor): number {
  switch (datum) {
    case 'amsl':
      return z
    case 'relative':
      return z + anchor.homeAmslM
    case 'agl':
      return z + anchor.homeAmslM
  }
}

/** Convert metres AMSL to a `z` expressed in `datum`. */
export function amslToZ(amsl: number, datum: AltitudeMode, anchor: AltitudeAnchor): number {
  switch (datum) {
    case 'amsl':
      return amsl
    case 'relative':
      return amsl - anchor.homeAmslM
    case 'agl':
      return amsl - anchor.homeAmslM
  }
}

/** Re-express one altitude when the datum changes. Exact round trip in `f64`. */
export function convertAltitudeZ(
  z: number,
  from: AltitudeMode,
  to: AltitudeMode,
  anchor: AltitudeAnchor,
): number {
  if (from === to) return z
  return amslToZ(zToAmsl(z, from, anchor), to, anchor)
}
