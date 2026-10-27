// Command risk grading (plan §7 U4): how much deliberate effort a command
// must cost before it is sent, so the confirmation style is one decision made
// in one place instead of a per-button habit.
//
//   low    — display / benign state: one click (e.g. switching to Hold)
//   medium — moves or arms the aircraft, but is recoverable: hold the button
//            for `HOLD_MS` while the ring fills (hold, not click — a stray tap
//            must not take off, land, RTL or arm)
//   high   — irreversible or leaves the vehicle uncontrollable (force-arm,
//            quit, disconnect, wiping the FC mission): the shell demands a
//            typed keyword on top (L4 `TeardownGuard`, L5 force-arm)
//
// The escalation matches the backend's own split: `command_priority` (A7)
// already marks RTL / LAND / ARM_DISARM as Emergency, and the two sets are
// deliberately not identical — priority is about the wire, risk is about the
// operator's hands.

import type { CommandName } from '../stores/command'

/** How much confirmation a command costs (plan §7 U4). */
export type CommandRisk = 'low' | 'medium' | 'high'

/** Risk of every command the flight menu can send. */
export const COMMAND_RISK: Record<CommandName, CommandRisk> = {
  takeoff: 'medium',
  land: 'medium',
  rtl: 'medium',
  arm_disarm: 'medium',
  arm: 'medium',
  disarm: 'medium',
  set_mode: 'low',
  goto: 'medium',
  set_home: 'medium',
}

/** Risk of a specific mode switch: the core table marks the disruptive modes
 * (`return`, `land`) with `confirm`, which is exactly the medium tier. */
export function modeRisk(confirm: boolean): CommandRisk {
  return confirm ? 'medium' : 'low'
}

/** The command's risk, defaulting to the cautious tier for anything unlisted. */
export function commandRisk(name: string): CommandRisk {
  return COMMAND_RISK[name as CommandName] ?? 'medium'
}
