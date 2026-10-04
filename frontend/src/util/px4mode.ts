// Decodes the PX4 custom_mode bitfield into a human-readable flight mode.
// The high byte packs the main mode, the next byte the sub-mode.
// See PX4 `commander` / `vehicle_status.h` for the canonical table.

const MAIN_MODES: Record<number, string> = {
  1: 'Manual',
  2: 'Altitude',
  3: 'Position',
  4: 'Auto',
  5: 'Acro',
  6: 'Offboard',
  7: 'Stabilized',
  8: 'Rattitude',
  9: 'Simple',
  10: 'VTOL Takeoff',
  11: 'VTOL Land',
  12: 'VTOL Mission',
  13: 'Rattitude',
}

const AUTO_SUB_MODES: Record<number, string> = {
  1: 'Loiter',
  2: 'Takeoff',
  3: 'RTL',
  4: 'RTML',
  5: 'Cam Abort',
  6: 'Precision Land',
  7: 'RTOD',
  8: 'VTOL',
  9: 'Mission',
  12: 'Ready',
  14: 'Fixed-Wing Takeoff',
  19: 'Follow Target',
}

/** Decode a PX4 custom_mode (u32) into a display string. */
export function px4Mode(customMode: number): string {
  const main = (customMode >>> 24) & 0xff
  const sub = (customMode >>> 16) & 0xff
  const mainName = MAIN_MODES[main]
  if (!mainName) return `Mode ${main}`
  if (main === 4 && AUTO_SUB_MODES[sub]) {
    return `${mainName}·${AUTO_SUB_MODES[sub]}`
  }
  return mainName
}