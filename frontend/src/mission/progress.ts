// Progress through the uploaded mission, for the readouts shown while flying
// (issues.md #37). The planning list says which waypoints exist; this says
// where the vehicle is on them.
//
// `MISSION_CURRENT.current` is the item the FC is *flying toward*, so that is
// the target. Everything degrades gracefully: without a telemetry fix or a
// MISSION_CURRENT the plan's own statistics are still returned.

import { bearingDeg, haversineM, type LatLon } from './geo'
import type { MissionItem } from '../generated-types/MissionItem'

export interface ProgressItem extends LatLon {
  seq: number
  /** Absolute AMSL altitude, metres. */
  altitude_m: number
}

export interface MissionProgress {
  total: number
  /** Index into `items` of the active target, or null when unknown. */
  targetIndex: number | null
  /** Items already passed (before the target). */
  flown: number
  /** Items still to fly, including the target. */
  remaining: number
  /** Straight-line distance from the vehicle to the target, metres. */
  toTargetM: number | null
  /** Bearing from the vehicle to the target, degrees clockwise from north. */
  toTargetBearingDeg: number | null
  /** Target altitude, metres AMSL. */
  targetAltitudeM: number | null
  /** Path length from the vehicle to the end of the mission, metres. */
  remainingPathM: number
  /** Path length already covered, metres. */
  flownPathM: number
  /** Fraction of the path covered, 0..1. */
  fraction: number
  /** Seconds to the end of the path at `groundSpeedMs`, or null. */
  etaS: number | null
}

/** Below this the vehicle is hovering; an ETA would be meaningless. */
const ETA_MIN_SPEED_MS = 0.5

function sumLegs(from: number, legs: readonly number[]): number {
  let total = 0
  for (let i = from; i < legs.length; i += 1) total += legs[i]
  return total
}

/**
 * Compute mission progress. `items` is the plan in flyable order (compiled
 * waypoints + complex-item children), `currentSeq` the FC's MISSION_CURRENT,
 * `vehicle` the latest position fix and `groundSpeedMs` the horizontal ground
 * speed used for the ETA.
 */
export function missionProgress(
  items: readonly ProgressItem[],
  currentSeq: number | null,
  vehicle: LatLon | null,
  groundSpeedMs: number | null,
): MissionProgress {
  const total = items.length
  const empty: MissionProgress = {
    total,
    targetIndex: null,
    flown: 0,
    remaining: total,
    toTargetM: null,
    toTargetBearingDeg: null,
    targetAltitudeM: null,
    remainingPathM: 0,
    flownPathM: 0,
    fraction: 0,
    etaS: null,
  }
  if (total === 0) return empty

  const legs: number[] = []
  for (let i = 0; i + 1 < total; i += 1) legs.push(haversineM(items[i], items[i + 1]))
  const totalPathM = legs.reduce((a, b) => a + b, 0)

  const targetIndex =
    currentSeq === null ? null : items.findIndex((item) => item.seq === currentSeq)
  const resolvedTarget = targetIndex !== null && targetIndex >= 0 ? targetIndex : null

  const pathFromTargetM = resolvedTarget === null ? totalPathM : sumLegs(resolvedTarget, legs)

  const toTargetM =
    vehicle !== null && resolvedTarget !== null ? haversineM(vehicle, items[resolvedTarget]) : null
  const toTargetBearingDeg =
    vehicle !== null && resolvedTarget !== null ? bearingDeg(vehicle, items[resolvedTarget]) : null

  const remainingPathM = toTargetM === null ? pathFromTargetM : toTargetM + pathFromTargetM
  const flownPathM = Math.max(0, totalPathM - remainingPathM)

  const fraction =
    totalPathM > 0
      ? Math.min(1, Math.max(0, flownPathM / totalPathM))
      : total > 0
        ? (resolvedTarget ?? 0) / total
        : 0

  const etaS =
    groundSpeedMs !== null && groundSpeedMs >= ETA_MIN_SPEED_MS
      ? remainingPathM / groundSpeedMs
      : null

  return {
    total,
    targetIndex: resolvedTarget,
    flown: resolvedTarget ?? 0,
    remaining: total - (resolvedTarget ?? 0),
    toTargetM,
    toTargetBearingDeg,
    targetAltitudeM: resolvedTarget === null ? null : items[resolvedTarget].altitude_m,
    remainingPathM,
    flownPathM,
    fraction,
    etaS,
  }
}

/** `m:ss` for an ETA in seconds, or null. */
export function formatDuration(seconds: number | null): string | null {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return null
  const total = Math.round(seconds)
  const m = Math.floor(total / 60)
  const s = total % 60
  return `${m}:${String(s).padStart(2, '0')}`
}


/**
 * Coordinate items of a compiled plan, in flyable order, ready for
 * [`missionProgress`]. Command items (`MAV_FRAME_MISSION`) carry arguments in
 * `x`/`y`, not a position, so they are skipped — `missionProgress` matches the
 * FC's MISSION_CURRENT by `seq`, which tolerates the resulting gaps.
 *
 * Callers pass items produced by `compile.ts::toDisplayItems`, so `x`/`y` are
 * always the ×1e7 global encoding.
 */
export function progressItemsOf(items: readonly MissionItem[]): ProgressItem[] {
  const out: ProgressItem[] = []
  for (const item of items) {
    if (item.frame === 'mission') continue
    out.push({
      seq: item.seq,
      latitude_deg: item.x / 1e7,
      longitude_deg: item.y / 1e7,
      altitude_m: item.z,
    })
  }
  return out
}


/** `412 m` under a kilometre, `1.84 km` above it. */
export function formatDistance(meters: number | null): string | null {
  if (meters === null || !Number.isFinite(meters) || meters < 0) return null
  if (meters < 1000) return `${Math.round(meters)} m`
  return `${(meters / 1000).toFixed(2)} km`
}
