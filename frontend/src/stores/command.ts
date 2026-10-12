// Flight command state (issue #5): tracks a command's round-trip through the
// desktop command service, which retransmits a COMMAND_LONG until the flight
// controller acks it. The terminal outcome arrives on the `"command"` event.

import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { logEvent } from './eventlog'

/**
 * Commands the UI may send. `takeoff`/`land` map to `MAV_CMD_NAV_TAKEOFF` /
 * `MAV_CMD_NAV_LAND`; `pause`/`continue` (`MAV_CMD_DO_PAUSE_CONTINUE`) were
 * removed — PX4 v1.17 has no handler for them (SITL-confirmed NACK).
 */
export type CommandName = 'takeoff' | 'land' | 'rtl' | 'set_mode' | 'arm_disarm'

/** Payload of the backend `command` event. */
export interface CommandEventPayload {
  command: string
  kind: 'sent' | 'completed' | 'failed'
  result?: string | null
  message?: string | null
}

interface CommandState {
  /** Command awaiting a terminal ack, or null when idle. */
  pending: CommandName | null
  /** Most recent event, kept for the status line. */
  lastEvent: CommandEventPayload | null
  send: (name: CommandName) => Promise<void>
  /** Switch the PX4 flight mode (MS3, ADR-017). */
  sendSetMode: (mainMode: number, subMode: number) => Promise<void>
  /** Arm or disarm the vehicle (MS3, ADR-017). */
  sendArmDisarm: (arm: boolean) => Promise<void>
  handleEvent: (e: CommandEventPayload) => void
  reset: () => void
}

export const useCommandStore = create<CommandState>((set) => ({
  pending: null,
  lastEvent: null,

  send: async (name) => {
    set({ pending: name, lastEvent: null })
    try {
      await invoke('send_command', { name })
    } catch (err) {
      set({
        pending: null,
        lastEvent: {
          command: name,
          kind: 'failed',
          result: null,
          message: String(err),
        },
      })
    }
  },

  sendSetMode: async (mainMode, subMode) => {
    set({ pending: 'set_mode', lastEvent: null })
    try {
      await invoke('send_set_mode', { mainMode, subMode })
    } catch (err) {
      set({
        pending: null,
        lastEvent: {
          command: 'set_mode',
          kind: 'failed',
          result: null,
          message: String(err),
        },
      })
    }
  },

  sendArmDisarm: async (arm) => {
    set({ pending: 'arm_disarm', lastEvent: null })
    try {
      await invoke('send_arm_disarm', { arm })
    } catch (err) {
      set({
        pending: null,
        lastEvent: {
          command: 'arm_disarm',
          kind: 'failed',
          result: null,
          message: String(err),
        },
      })
    }
  },

  // `sent` (including retransmissions) keeps the command pending; a
  // `completed`/`failed` event always ends it.
  handleEvent: (e) => {
    // Event log (operator request): the nav bar no longer shows the ack
    // inline; every terminal outcome lands in the log instead.
    if (e.kind !== 'sent') {
      const level = e.result === 'accepted' ? 'ok' : e.result === undefined ? 'err' : 'warn'
      logEvent('command', level, `${e.command}: ${e.result ?? e.message ?? 'failed'}`)
    }
    set((s) => ({
      lastEvent: e,
      pending: e.kind === 'sent' ? s.pending : null,
    }))
  },

  reset: () => set({ pending: null, lastEvent: null }),
}))
