// Great-circle geometry for the flight readouts (WS-G G3): distance to the
// next waypoint, remaining path length and bearing.
//
// Spherical (haversine) maths is enough at survey scales — the error against a
// WGS84 geodesic is ~0.3%, i.e. well under a metre per 300 m — and it needs no
// projection state, so it can run on every telemetry frame.

/** Mean Earth radius (IUGG), metres. */
export const EARTH_RADIUS_M = 6371008.8

export interface LatLon {
  latitude_deg: number
  longitude_deg: number
}

const DEG = Math.PI / 180

/** Normalise an angle to [0, 360). */
export function normalizeDeg(deg: number): number {
  const d = deg % 360
  return d < 0 ? d + 360 : d
}

/** Great-circle distance between two points, metres. */
export function haversineM(a: LatLon, b: LatLon): number {
  const lat1 = a.latitude_deg * DEG
  const lat2 = b.latitude_deg * DEG
  const dLat = lat2 - lat1
  const dLon = (b.longitude_deg - a.longitude_deg) * DEG
  const s = Math.sin(dLat / 2) ** 2 + Math.cos(lat1) * Math.cos(lat2) * Math.sin(dLon / 2) ** 2
  return 2 * EARTH_RADIUS_M * Math.asin(Math.min(1, Math.sqrt(s)))
}

/** Initial true bearing from `a` to `b`, degrees clockwise from north. */
export function bearingDeg(a: LatLon, b: LatLon): number {
  const lat1 = a.latitude_deg * DEG
  const lat2 = b.latitude_deg * DEG
  const dLon = (b.longitude_deg - a.longitude_deg) * DEG
  const y = Math.sin(dLon) * Math.cos(lat2)
  const x = Math.cos(lat1) * Math.sin(lat2) - Math.sin(lat1) * Math.cos(lat2) * Math.cos(dLon)
  return normalizeDeg(Math.atan2(y, x) / DEG)
}
