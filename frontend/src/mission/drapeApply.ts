// Plan application for the A9 drape pipeline (ADR-022): the backend
// `drape_preview` solves + compiles the pattern lines and returns the route
// in the stored AMSL datum plus the DEM-surface heights for the landing
// items. This module merges both into the editable plan — pure, so
// `scripts/check-agl.mjs` can assert the merge semantics directly.
//
// What is replaced: the pattern block (the compiled route, speed items
// included). What survives: pinned heights inside the block (DR1: the pin
// survives every re-drape — restored onto the matching compiled waypoint),
// landing items get the DEM surface (no AGL, no vertical offset), and
// everything outside the block passes through untouched.

import type { PlannedWaypoint } from '../generated-types/PlannedWaypoint'
import type { CompiledWaypoint } from '../generated-types/CompiledWaypoint'

/** Coordinate command items that must finish *on* the ground:
 * `NAV_LAND` / `NAV_VTOL_LAND`. */
const LAND_COMMANDS: readonly number[] = [21, 85]

/** Position key with f32-safe precision: the compiled positions round-trip
 * through the backend (17-digit JSON), so exact equality is not guaranteed. */
function posKey(p: { latitude_deg: number; longitude_deg: number }): string {
  return `${p.latitude_deg.toFixed(9)},${p.longitude_deg.toFixed(9)}`
}

/** The inclusive seq range the generated pattern block occupies. */
export interface DrapeBlock {
  start: number
  end: number
}

export interface DrapeApplyResult {
  waypoints: PlannedWaypoint[]
  /** Compiled route items written (speed items included). */
  applied: number
  /** Landing items placed on the DEM surface. */
  landed: number
  /** Pinned heights restored over the compiled route. */
  pinned: number
  /** Where the block sits after the swap (the route length decides). */
  blockStart: number
  blockEnd: number
  /** Re-anchored line table: one row per compiled line, in order. */
  lines: Array<{ kind: string; start_seq: number; end_seq: number }>
}

/**
 * Replace the pattern block with the compiled route and set the landing
 * heights. Inputs are never mutated.
 */
export function applyCompiled(
  current: readonly PlannedWaypoint[],
  block: DrapeBlock,
  compiled: readonly CompiledWaypoint[],
  landingAmslByPosition: ReadonlyMap<string, number>,
): DrapeApplyResult {
  const start = Math.max(0, Math.min(block.start, current.length))
  const end = Math.max(start - 1, Math.min(block.end, current.length - 1))

  // Pinned heights inside the block survive: the compiled route carries the
  // solver's height for the same position, the operator's hand-set one wins.
  const pins = new Map<string, PlannedWaypoint>()
  for (let i = start; i <= end; i += 1) {
    const wp = current[i]
    if (wp.locked) pins.set(posKey(wp.position), wp)
  }

  const route: PlannedWaypoint[] = compiled.map((c) => {
    const wp: PlannedWaypoint = {
      ...c.waypoint,
      altitude: { ...c.waypoint.altitude },
    }
    const pin = pins.get(posKey(wp.position))
    if (pin) {
      return { ...wp, altitude: { ...pin.altitude }, locked: true }
    }
    return wp
  })
  const pinned = route.filter((wp) => wp.locked).length

  // Pass-through outside the block: command items, pinned waypoints, manual
  // waypoints that are not part of the generated pattern.
  const out: PlannedWaypoint[] = [
    ...current.slice(0, start),
    ...route,
    ...current.slice(end + 1),
  ]

  // Landing items aim at the DEM surface itself: neither the survey AGL nor
  // the DSM clearance applies (operator report: a landing target left on the
  // HOME plane left the vehicle hovering over lower ground).
  let landed = 0
  for (let i = 0; i < out.length; i += 1) {
    const wp = out[i]
    if (wp.locked || !LAND_COMMANDS.includes(wp.command)) continue
    const ground = landingAmslByPosition.get(posKey(wp.position))
    if (ground == null || !Number.isFinite(ground)) continue
    out[i] = { ...wp, altitude: { ...wp.altitude, meters: ground } }
    landed += 1
  }

  // Re-anchor the line table: the compiled items appear in line order, one
  // speed item riding at each line start. The kind rides in the generated
  // line id (`survey-3` → `survey`).
  const lines: DrapeApplyResult['lines'] = []
  let cursor = start
  let currentId: string | null = null
  let currentStart = start
  const flush = () => {
    if (currentId !== null) {
      lines.push({
        kind: currentId.split('-')[0] ?? currentId,
        start_seq: currentStart,
        end_seq: cursor - 1,
      })
    }
  }
  for (const c of compiled) {
    if (c.line_id !== currentId) {
      flush()
      currentId = c.line_id
      currentStart = cursor
    }
    cursor += 1
  }
  flush()

  return {
    waypoints: out,
    applied: route.length,
    landed,
    pinned,
    blockStart: start,
    blockEnd: start + route.length - 1,
    lines,
  }
}
