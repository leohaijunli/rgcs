// Which pattern line each waypoint belongs to, and how to split a plan into
// coloured runs (the survey/tie/calibration readout on the map).
//
// `core::survey` returns a `PatternLine` table (seq ranges per kind) next to the
// generated waypoints; the store keeps it in `lastPattern`. This module turns
// that table into a `seq -> kind` map and groups consecutive same-kind items, so
// the map can draw one polyline per run instead of a single coloured path.

import type { LineKind } from '../generated-types/LineKind'
import type { PatternLine } from '../generated-types/PatternLine'

/** Map of waypoint `seq` to the kind of line it belongs to. */
export type KindBySeq = Map<number, LineKind>

/** Flatten a pattern's line table into a per-seq lookup. */
export function kindsBySeq(lines: readonly PatternLine[]): KindBySeq {
  const kinds: KindBySeq = new Map()
  for (const line of lines) {
    const first = Math.min(line.start_seq, line.end_seq)
    const last = Math.max(line.start_seq, line.end_seq)
    for (let seq = first; seq <= last; seq += 1) kinds.set(seq, line.kind)
  }
  return kinds
}

export interface Run<T> {
  kind: LineKind | null
  items: T[]
}

/**
 * Group consecutive items that share a line kind, so each run can be drawn in
 * its own colour. Items without a kind (hand-placed waypoints) form `null` runs.
 * A run never spans across a gap in the kind map, so a tie line stays visually
 * separate from the survey lines it links.
 */
export function splitRuns<T extends { seq: number }>(
  items: readonly T[],
  kinds: KindBySeq,
): Run<T>[] {
  const runs: Run<T>[] = []
  for (const item of items) {
    const kind = kinds.get(item.seq) ?? null
    const last = runs[runs.length - 1]
    if (last && last.kind === kind) {
      last.items.push(item)
      continue
    }
    runs.push({ kind, items: [item] })
  }
  return runs
}

/**
 * Vertical extent to draw for each item: a stick from the ground up to the
 * waypoint.
 *
 * `groundM` is the ground AMSL altitude to stand the sticks on — HOME's
 * altitude while there is no DEM, the same flat-ground assumption the AGL mode
 * makes (WS-D). Without it (no home anchor) the plan's own lowest waypoint is
 * used, which still answers "which waypoints are higher?".
 */
export function heightRuns<T extends { seq: number; z: number }>(
  items: readonly T[],
  groundM: number | null = null,
): Array<{ seq: number; base: number; top: number }> {
  const coordinates = items.filter((item) => Number.isFinite(item.z))
  if (coordinates.length === 0) return []
  const base = groundM ?? Math.min(...coordinates.map((item) => item.z))
  return coordinates.map((item) => ({ seq: item.seq, base, top: item.z }))
}
