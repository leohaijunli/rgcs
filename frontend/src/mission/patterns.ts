// Preset pattern generation (ADR-013 survey patterns).
//
// The geometry lives in `core::survey`; the frontend builds the parameters and
// calls the two Tauri commands. Generated waypoints are appended to the plan as
// ordinary editable AMSL waypoints, and the returned line table is kept for the
// segmentation readout.

import { invoke } from '@tauri-apps/api/core'
import type { CloverleafPattern } from '../generated-types/CloverleafPattern'
import type { GeoPoint } from '../generated-types/GeoPoint'
import type { PatternPlan } from '../generated-types/PatternPlan'
import type { SurveyPattern } from '../generated-types/SurveyPattern'

/** Generate a survey sweep (optionally with tie lines). */
export async function generateSweep(pattern: SurveyPattern): Promise<PatternPlan> {
  return invoke<PatternPlan>('survey_generate_sweep', { pattern })
}

/** Generate a cloverleaf calibration manoeuvre. */
export async function generateCloverleaf(pattern: CloverleafPattern): Promise<PatternPlan> {
  return invoke<PatternPlan>('survey_generate_cloverleaf', { pattern })
}

/** WGS84 semi-major axis, matching `core::survey::LocalProjection`. */
const SEMI_MAJOR_M = 6_378_137
const DEG_TO_RAD = Math.PI / 180

/** Offset a centre by east/north metres (local tangent plane). */
function offset(center: GeoPoint, eastM: number, northM: number): GeoPoint {
  const lat = center.latitude_deg + northM / (SEMI_MAJOR_M * DEG_TO_RAD)
  const cos = Math.cos(center.latitude_deg * DEG_TO_RAD)
  const lon = center.longitude_deg + eastM / (SEMI_MAJOR_M * cos * DEG_TO_RAD)
  return { latitude_deg: lat, longitude_deg: lon }
}

/** Rectangle (CCW), centred on `center`, sized in metres east/north. */
export function rectanglePolygon(center: GeoPoint, widthEW: number, heightNS: number): GeoPoint[] {
  const e = widthEW / 2
  const n = heightNS / 2
  return [
    offset(center, -e, -n),
    offset(center, e, -n),
    offset(center, e, n),
    offset(center, -e, n),
  ]
}

/** Default sweep parameters over a square around `center`. */
export function defaultSweep(center: GeoPoint, altitudeAmslM: number): SurveyPattern {
  return {
    polygon: rectanglePolygon(center, 500, 500),
    line_azimuth_deg: 0,
    line_spacing_m: 50,
    tie_spacing_m: 250,
    tie_azimuth_deg: 90,
    lead_in_m: 20,
    lead_out_m: 20,
    altitude_amsl_m: altitudeAmslM,
    alternate: true,
    speed_mps: 5,
  }
}

/** Default cloverleaf parameters around `center`. */
export function defaultCloverleaf(center: GeoPoint, altitudeAmslM: number): CloverleafPattern {
  return {
    center,
    radius_m: 75,
    petals: 4,
    samples_per_petal: 10,
    altitude_amsl_m: altitudeAmslM,
    start_heading_deg: 0,
    speed_mps: 5,
  }
}
