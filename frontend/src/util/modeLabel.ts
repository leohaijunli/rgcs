// Display name of the current flight mode.
//
// PX4 modes arrive pre-decoded from core (`Heartbeat.mode_label`,
// core::telemetry::px4_mode, ADR-017) so the bitfield decode lives in
// exactly one place; for any other autopilot we only have the MAV_STATE
// system status, which is a coarse state rather than a mode.

import type { Heartbeat } from '../generated-types/Heartbeat'

type Translate = (key: string) => string

export function modeLabel(t: Translate, hb: Heartbeat | null): string | null {
  if (!hb) return null
  if (hb.autopilot === 'px4') return hb.mode_label || null
  return t(`mode.${hb.flight_state.toLowerCase()}`)
}
