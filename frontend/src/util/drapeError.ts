// Backend DEM drape failures (ADR-004: the core reader fails closed on the
// first waypoint outside the DEM or on a void) are too terse to act on. This
// maps them to one message the planning panel can pair with a fix, keeping the
// raw backend strings ("point outside the DEM coverage" / "DEM void") off the
// screen. Anything else passes through unchanged.

/** A waypoint falls outside the loaded DEM (or on a void). */
export const DEM_NOT_COVERED_ERROR =
  'a waypoint is outside the loaded DEM (or on a void) — load a DEM that covers the whole survey area, or shrink the plan'

export function drapeErrorReason(raw: string): string {
  const m = raw.toLowerCase()
  if (m.includes('outside the dem coverage') || m.includes('dem void')) {
    return DEM_NOT_COVERED_ERROR
  }
  return raw
}
