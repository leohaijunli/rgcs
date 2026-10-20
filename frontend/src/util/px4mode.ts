// PX4 custom-mode packing (ADR-017) shared by the mode popover and the
// always-visible Hold button (plan §4 L2): `main_mode` in bits 16–23,
// `sub_mode` in bits 24–31. The numbers mirror
// `core::telemetry::px4_mode::{main_mode, auto_sub}` — the frontend never
// decodes a mode itself, it only builds the two the UI offers by name.

/** `PX4_CUSTOM_MAIN_MODE_AUTO`. */
export const MAIN_MODE_AUTO = 4
/** `PX4_CUSTOM_SUB_MODE_AUTO_LOITER` — the mode table's `hold` entry. */
export const AUTO_SUB_LOITER = 3

/** Pack a PX4 `custom_mode` for `HEARTBEAT` comparisons / `DO_SET_MODE`. */
export function packCustomMode(main: number, sub: number): number {
  return ((main & 0xff) << 16) | ((sub & 0xff) << 24)
}

/** The `custom_mode` a switch to Hold lands on. */
export const HOLD_CUSTOM_MODE = packCustomMode(MAIN_MODE_AUTO, AUTO_SUB_LOITER)
