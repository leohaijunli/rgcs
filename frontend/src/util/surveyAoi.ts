// The survey-area AOI for DEM work (HRDEM fetch, 3D mesh, prepared-data
// status): where the centre is and how wide/tall the window must be.
//
// The boundary comes from one of two sources — a drawn polygon (the polygon
// map tool) or the manual centre + width/height rectangle. The DEM must cover
// the **whole boundary**, so a closed polygon wins and its axis-aligned
// bounding box (in degrees) is the window; the rectangle inputs are the
// fallback. The backend adds its own margin on every side (plan decision 3,
// 2026-10-18 revision), so no extra padding is added here.

import type { GeoPoint } from '../generated-types/GeoPoint'

/** WGS84 semi-major axis (metres), matching `core::survey::LocalProjection`. */
const SEMI_MAJOR_M = 6_378_137
const DEG_TO_RAD = Math.PI / 180

/** Centre + window size for a DEM fetch, in degrees / metres. */
export interface SurveyAoi {
  lat: number
  lon: number
  width: number
  height: number
}

/** Axis-aligned bounding box of `vertices`, in metres at the box's mid-latitude. */
export function polygonAoi(vertices: readonly GeoPoint[]): SurveyAoi | null {
  if (vertices.length < 3) return null
  const lats = vertices.map((v) => v.latitude_deg)
  const lons = vertices.map((v) => v.longitude_deg)
  const south = Math.min(...lats)
  const north = Math.max(...lats)
  const west = Math.min(...lons)
  const east = Math.max(...lons)
  const cos = Math.cos(((south + north) / 2) * DEG_TO_RAD)
  return {
    lat: (south + north) / 2,
    lon: (west + east) / 2,
    width: (east - west) * DEG_TO_RAD * SEMI_MAJOR_M * cos,
    height: (north - south) * DEG_TO_RAD * SEMI_MAJOR_M,
  }
}

/** The AOI the DEM work must cover: the drawn polygon's bounding box when a
 * closed boundary exists, else the rectangle around `center` (which may be
 * null — the caller then has no AOI at all). */
export function effectiveSurveyAoi(
  polygon: { vertices: readonly GeoPoint[]; closed: boolean } | null,
  center: GeoPoint | null,
  size: { width: number; height: number },
): SurveyAoi | null {
  if (polygon?.closed) {
    const box = polygonAoi(polygon.vertices)
    if (box) return box
  }
  if (!center) return null
  return { lat: center.latitude_deg, lon: center.longitude_deg, width: size.width, height: size.height }
}
