// Flight command state (issue #5): tracks a command's round-trip through the
// desktop command service, which retransmits a COMMAND_LONG until the flight
// controller acks it. The terminal outcome arrives on the `"command"` event.

import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { logEvent } from './eventlog'
import { useTelemetryStore } from './telemetry'

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
  /** Fly the vehicle to a map point (`MAV_CMD_DO_REPOSITION`). */
  sendGoto: (latDeg: number, lonDeg: number) => Promise<void>
  /** Set the vehicle's home to a map point (`MAV_CMD_DO_SET_HOME`). */
  sendSetHome: (latDeg: number, lonDeg: number) => Promise<void>
  /** Map point of the last "Move to here" click, shown as a marker until the
   * vehicle arrives or the command fails. */
  gotoTarget: [number, number] | null
  clearGotoTarget: () => void
  /** HOME the vehicle accepted via "Set home" (lat, lon, AMSL m): the HOME
   * marker follows it, since there is no HOME_POSITION feed. */
  homeMarker: [number, number, number] | null
  handleEvent: (e: CommandEventPayload) => void
  reset: () => void
}

/** Set-home request awaiting its ack; promoted to `homeMarker` on accept. */
let pendingHome: [number, number, number] | null = null

export const useCommandStore = create<CommandState>((set) => ({
  pending: null,
  lastEvent: null,
  gotoTarget: null,
  homeMarker: null,
  clearGotoTarget: () => set({ gotoTarget: null }),

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

  /** Fly the vehicle to a map point (`MAV_CMD_DO_REPOSITION`). */
  sendGoto: async (latDeg, lonDeg) => {
    set({ gotoTarget: [latDeg, lonDeg] })
    try {
      await invoke('send_goto', { latDeg, lonDeg })
    } catch (err) {
      // Invoke-level failures ("not connected", "no position fix yet") never
      // reach the backend `command` event, so log them here or they vanish.
      logEvent('command', 'err', `goto: ${String(err)}`)
      set({
        gotoTarget: null,
        pending: null,
        lastEvent: {
          command: 'goto',
          kind: 'failed',
          result: null,
          message: String(err),
        },
      })
    }
  },

  /** Set the vehicle's home to a map point (`MAV_CMD_DO_SET_HOME`). */
  sendSetHome: async (latDeg, lonDeg) => {
    // Same altitude the backend sends (the current AMSL fix).
    const alt = useTelemetryStore.getState().snapshot?.global_position?.altitude.meters ?? 0
    pendingHome = [latDeg, lonDeg, alt]
    try {
      await invoke('send_set_home', { latDeg, lonDeg })
    } catch (err) {
      // Invoke-level failures ("not connected", "no position fix yet") never
      // reach the backend `command` event, so log them here or they vanish.
      logEvent('command', 'err', `set_home: ${String(err)}`)
      pendingHome = null
      set({
        pending: null,
        lastEvent: {
          command: 'set_home',
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
    // Marker bookkeeping for the two map-click commands.
    if (e.kind !== 'sent') {
      if (e.command === 'goto' && e.result !== 'accepted') set({ gotoTarget: null })
      if (e.command === 'set_home') {
        if (e.result === 'accepted' && pendingHome) set({ homeMarker: pendingHome })
        pendingHome = null
      }
    }
    set((s) => ({
      lastEvent: e,
      pending: e.kind === 'sent' ? s.pending : null,
    }))
  },

  reset: () => {
    pendingHome = null
    set({ pending: null, lastEvent: null, gotoTarget: null, homeMarker: null })
  },
}))
