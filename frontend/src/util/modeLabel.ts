// Display name of the current flight mode.
//
// PX4 packs its mode into `custom_mode`; for any other autopilot we only have
// the MAV_STATE system status, which is a coarse state rather than a mode.

import type { Heartbeat } from '../generated-types/Heartbeat'
import { px4Mode } from './px4mode'

type Translate = (key: string) => string

export function modeLabel(t: Translate, hb: Heartbeat | null): string | null {
  if (!hb) return null
  if (hb.autopilot === 'px4') return px4Mode(hb.custom_mode)
  return t(`mode.${hb.flight_state.toLowerCase()}`)
}
