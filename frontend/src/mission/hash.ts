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
 *
 * `x`/`y` of a `MAV_FRAME_MISSION` item must be narrowed too, but for a
 * different reason: PX4 does not store command items verbatim — it parks their
 * coordinate fields in the internal `mission_item_s.params[4..6]`, which are
 * `f32` (PX4 `mavlink_mission.cpp`, `to_mavlink_mission_item` /
 * `copy_params_from_mavlink_to_mission_item`). A command item that carries a
 * reference position (the sweep's `DO_CHANGE_SPEED`, seq 0) uploads
 * `x = 486493000` and the read-back echoes `round(f32(486493000)) = 486492992`,
 * so an exact comparison always fails. `Math.fround` applies the same
 * narrowing the FC applies. Global-frame `x`/`y` are exact `i32` on the wire
 * and stay uncompared at full precision.
 */
export function itemsHash(items: MissionItem[]): string {
  return JSON.stringify(
    items.map((i) => [
      i.frame,
      i.command,
      i.params.map(Math.fround),
      i.frame === 'mission' ? Math.fround(i.x) : i.x,
      i.frame === 'mission' ? Math.fround(i.y) : i.y,
      Math.fround(i.z),
      i.autocontinue,
    ]),
  )
}
