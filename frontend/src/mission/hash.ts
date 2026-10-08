// Structural hash used to compare a plan with what the FC reports.

import type { MissionItem } from '../generated-types/MissionItem'

/**
 * `seq` is derived from the array index on the wire and `current` is set by the
 * FC for whichever item it happens to be flying, so neither is part of the
 * plan's identity: hashing them caused false "mismatch with FC" reports
 * (finding 5). Order is already encoded by the JSON array.
 *
 * `z` and `params` are `f32` on the wire (`core::mission::MissionItem`), so a
 * plan imported from a `.plan` file at f64 precision must be narrowed before
 * the comparison — otherwise every upload reports "FC mission differs from the
 * uploaded plan" even though the FC holds exactly the plan we sent
 * (issues.md #38).
 */
export function itemsHash(items: MissionItem[]): string {
  return JSON.stringify(
    items.map((i) => [
      i.frame,
      i.command,
      i.params.map(Math.fround),
      i.x,
      i.y,
      Math.fround(i.z),
      i.autocontinue,
    ]),
  )
}
