// App-layer mirror of `terrain_service::DrapePreview` (the Tauri command's
// payload; the app layer has no ts-rs, so the type is hand-written here).

import type { DrapeResult } from '../generated-types/DrapeResult'
import type { PlannedWaypoint } from '../generated-types/PlannedWaypoint'

/** One plan line for the drape preview. */
export interface DrapeLineInput {
  id: string
  /** `survey` | `tie` | `calibration` (unknown degrades to survey). */
  kind: string
  /** `[lon, lat]` pairs in WGS84 degrees. */
  vertices: Array<[number, number]>
}

/** The solver + compiler result (A9: the only waypoint producer), the
 * route the plan stores (AMSL), and the DEM facts the checklist group and
 * the planning UI need. */
export interface DrapePreview {
  /** DEM-datum result: the monitor baseline's frame (`result.compiled`). */
  result: DrapeResult
  /** The compiled route in the stored datum (`AMSL_EGM96`), fly order. */
  waypoints: PlannedWaypoint[]
  /** DEM-surface AMSL height per landing point passed in (input order). */
  landing_amsl_m: number[]
  has_dtm: boolean
  acquisition: string | null
  resolution_m: number | null
  covers_route: boolean
  dem_age_ok: boolean | null
}
