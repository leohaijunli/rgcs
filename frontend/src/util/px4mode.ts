// Decodes the PX4 custom_mode bitfield into a human-readable flight mode.
//
// Layout is unchanged in PX4 v1.17 (src/modules/commander/px4_custom_mode.h):
//   uint16 reserved; uint8 main_mode; uint8 sub_mode;
// so on the wire main_mode is bits 16-23 and sub_mode is bits 24-31.

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
  10: 'Termination',
  11: 'Altitude Cruise',
}

const AUTO_SUB_MODES: Record<number, string> = {
  1: 'Ready',
  2: 'Takeoff',
  3: 'Loiter',
  4: 'Mission',
  5: 'RTL',
  6: 'Land',
  8: 'Follow Target',
  9: 'Precision Land',
  10: 'VTOL Takeoff',
}

const POSCTL_SUB_MODES: Record<number, string> = {
  1: 'Orbit',
  2: 'Slow',
}

/** Decode a PX4 custom_mode (u32) into a display string. */
export function px4Mode(customMode: number): string {
  const main = (customMode >>> 16) & 0xff
  const sub = (customMode >>> 24) & 0xff
  const mainName = MAIN_MODES[main]
  if (!mainName) return `Mode ${main}`
  const subName = main === 4 ? AUTO_SUB_MODES[sub] : main === 3 ? POSCTL_SUB_MODES[sub] : undefined
  return subName ? `${mainName}·${subName}` : mainName
}
