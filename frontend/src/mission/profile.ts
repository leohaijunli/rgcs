// Plan altitude profile series (plan review): the planned height against the
// DEM ground under it, **sampled along the flown path** — not only at the
// waypoints. Survey patterns place waypoints at line ends tens to hundreds of
// metres apart, so a waypoint-only polyline renders a 20 km survey as a few
// dozen points and hides every undulation between them.
//
// Pure and synchronous: the ground series interpolates the per-waypoint DEM
// samples (`groundBySeq`, keyed by the waypoint's index — the same key the
// map's ground sampling uses) between neighbours, and stays null across a gap
// (the chart bridges with `spanGaps`).

import type { PlannedWaypoint } from '../generated-types/PlannedWaypoint'

/** MAV_CMD value that carries a coordinate (`NAV_WAYPOINT`). */
const NAV_WAYPOINT = 16

/** Cap on profile samples — enough to smooth a 20 km path at ~10 m, cheap for
 * uPlot to redraw on every plan edit. */
const MAX_SAMPLES = 2000

/** Minimum sample count for a short plan (keeps short paths from collapsing
 * to their waypoints only). */
const MIN_SAMPLES = 100

/** Sample spacing target, metres. */
const STEP_M = 10

export interface ProfileSeries {
  /** Along-track distance from the start, metres (ascending). */
  dist: number[]
  /** Planned AMSL at each sample. */
  amsl: number[]
  /** Interpolated DEM ground at each sample; null across unsampled gaps. */
  ground: (number | null)[]
  /** Planned height minus ground; null where the ground is unknown. */
  agl: (number | null)[]
}

/** Great-circle distance in metres between two WGS84 degree points. */
function haversineM(
  a: { latitude_deg: number; longitude_deg: number },
  b: { latitude_deg: number; longitude_deg: number },
): number {
  const R = 6_371_000
  const dLat = ((b.latitude_deg - a.latitude_deg) * Math.PI) / 180
  const dLon = ((b.longitude_deg - a.longitude_deg) * Math.PI) / 180
  const s =
    Math.sin(dLat / 2) ** 2 +
    Math.cos((a.latitude_deg * Math.PI) / 180) *
      Math.cos((b.latitude_deg * Math.PI) / 180) *
      Math.sin(dLon / 2) ** 2
  return 2 * R * Math.asin(Math.sqrt(s))
}

const lerp = (a: number, b: number, t: number): number => a + (b - a) * t

/** Build the profile series, or `null` when fewer than two flyable waypoints. */
export function profileSeries(
  waypoints: readonly PlannedWaypoint[],
  groundBySeq: Readonly<Record<number, number>> | null,
): ProfileSeries | null {
  // Keep the waypoint's index in the full list: that index is the `seq` the
  // compiled mission items (and `groundBySeq`) use. Filtering first would
  // shift every ground sample as soon as a takeoff/landing item exists.
  const pts: { dist: number; amsl: number; ground: number | null }[] = []
  let prev: PlannedWaypoint | null = null
  waypoints.forEach((wp, seq) => {
    if (wp.command !== NAV_WAYPOINT) return
    const d = prev ? haversineM(prev.position, wp.position) : 0
    pts.push({
      dist: (pts.length > 0 ? pts[pts.length - 1].dist : 0) + d,
      amsl: wp.altitude.meters,
      ground: groundBySeq?.[seq] ?? null,
    })
    prev = wp
  })
  if (pts.length < 2) return null

  const total = pts[pts.length - 1].dist
  const count = Math.min(MAX_SAMPLES, Math.max(MIN_SAMPLES, Math.ceil(total / STEP_M)))
  const step = total / count

  const dist: number[] = []
  const amsl: number[] = []
  const ground: (number | null)[] = []
  const agl: (number | null)[] = []
  let seg = 0
  for (let i = 0; i <= count; i++) {
    const d = i === count ? total : i * step
    while (seg < pts.length - 2 && pts[seg + 1].dist < d) seg += 1
    const a = pts[seg]
    const b = pts[seg + 1]
    const span = b.dist - a.dist
    const t = span > 0 ? (d - a.dist) / span : 0
    const g = a.ground != null && b.ground != null ? lerp(a.ground, b.ground, t) : null
    const h = lerp(a.amsl, b.amsl, t)
    dist.push(d)
    amsl.push(h)
    ground.push(g)
    agl.push(g != null ? h - g : null)
  }
  return { dist, amsl, ground, agl }
}
