// Structural hash used to compare a plan with what the FC reports.

import type { MissionItem } from '../generated-types/MissionItem'

/**
 * `seq` is derived from the array index on the wire and `current` is set by the
 * FC for whichever item it happens to be flying, so neither is part of the
 * plan's identity: hashing them caused false "mismatch with FC" reports
 * (finding 5). Order is already encoded by the JSON array.
 */
export function itemsHash(items: MissionItem[]): string {
  return JSON.stringify(
    items.map((i) => [i.frame, i.command, i.params, i.x, i.y, i.z, i.autocontinue]),
  )
}
