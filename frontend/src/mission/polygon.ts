// Survey-polygon geometry: area, perimeter and simplicity (self-intersection).
//
// Mirrors `core::survey::validate_polygon` and the local tangent-plane
// projection, so the map tool can flag a bad boundary before Generate reaches
// Rust. The projection origin is the first vertex: area/perimeter are
// translation-invariant, and the orientation signs the self-intersection check
// needs are scale-invariant.

import type { GeoPoint } from '../generated-types/GeoPoint'

/** WGS84 semi-major axis (metres), matching `core::survey::LocalProjection`. */
const SEMI_MAJOR_M = 6_378_137
const DEG_TO_RAD = Math.PI / 180

/** Local (east, north) metres around `origin`. */
function toLocal(origin: GeoPoint, p: GeoPoint): [number, number] {
  const cos = Math.cos(origin.latitude_deg * DEG_TO_RAD)
  const east = (p.longitude_deg - origin.longitude_deg) * DEG_TO_RAD * SEMI_MAJOR_M * cos
  const north = (p.latitude_deg - origin.latitude_deg) * DEG_TO_RAD * SEMI_MAJOR_M
  return [east, north]
}

function project(vertices: readonly GeoPoint[]): [number, number][] {
  if (vertices.length === 0) return []
  // The centroid origin makes the geometry winding- and order-independent:
  // both directions project onto the same tangent plane.
  const n = vertices.length
  const lat = vertices.reduce((s, v) => s + v.latitude_deg, 0) / n
  const lon = vertices.reduce((s, v) => s + v.longitude_deg, 0) / n
  const origin = { latitude_deg: lat, longitude_deg: lon }
  return vertices.map((p) => toLocal(origin, p))
}

/** Absolute polygon area in square metres (shoelace, local projection). */
export function polygonArea(vertices: readonly GeoPoint[]): number {
  const pts = project(vertices)
  if (pts.length < 3) return 0
  let twice = 0
  for (let i = 0; i < pts.length; i += 1) {
    const a = pts[i]
    const b = pts[(i + 1) % pts.length]
    twice += a[0] * b[1] - b[0] * a[1]
  }
  return Math.abs(twice) / 2
}

/** Polygon perimeter in metres (including the closing edge). */
export function polygonPerimeter(vertices: readonly GeoPoint[]): number {
  const pts = project(vertices)
  let sum = 0
  for (let i = 0; i < pts.length; i += 1) {
    const a = pts[i]
    const b = pts[(i + 1) % pts.length]
    sum += Math.hypot(b[0] - a[0], b[1] - a[1])
  }
  return sum
}

/** Whether the polygon is not simple: two non-adjacent edges cross or touch. */
export function selfIntersects(vertices: readonly GeoPoint[]): boolean {
  if (vertices.length < 4) return false
  const pts = project(vertices)
  const n = pts.length
  for (let i = 0; i < n; i += 1) {
    for (let j = i + 1; j < n; j += 1) {
      if (j === i + 1 || (i === 0 && j === n - 1)) continue
      if (segmentsIntersect(pts[i], pts[(i + 1) % n], pts[j], pts[(j + 1) % n])) return true
    }
  }
  return false
}

/** Collinearity/touch tolerance in metres (1e-9 ≈ 1 nm at survey scale). */
const EPS_M = 1e-9

function orient(a: [number, number], b: [number, number], c: [number, number]): number {
  return (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

function onSegment(a: [number, number], b: [number, number], c: [number, number]): boolean {
  return (
    c[0] >= Math.min(a[0], b[0]) - EPS_M &&
    c[0] <= Math.max(a[0], b[0]) + EPS_M &&
    c[1] >= Math.min(a[1], b[1]) - EPS_M &&
    c[1] <= Math.max(a[1], b[1]) + EPS_M
  )
}

function segmentsIntersect(
  a: [number, number],
  b: [number, number],
  c: [number, number],
  d: [number, number],
): boolean {
  const o1 = orient(a, b, c)
  const o2 = orient(a, b, d)
  const o3 = orient(c, d, a)
  const o4 = orient(c, d, b)
  const proper = (o1 > EPS_M && o2 < -EPS_M) || (o1 < -EPS_M && o2 > EPS_M)
  if (proper && ((o3 > EPS_M && o4 < -EPS_M) || (o3 < -EPS_M && o4 > EPS_M))) {
    return true
  }
  if (Math.abs(o1) <= EPS_M && onSegment(a, b, c)) return true
  if (Math.abs(o2) <= EPS_M && onSegment(a, b, d)) return true
  if (Math.abs(o3) <= EPS_M && onSegment(c, d, a)) return true
  if (Math.abs(o4) <= EPS_M && onSegment(c, d, b)) return true
  return false
}