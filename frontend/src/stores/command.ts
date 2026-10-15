// Flight command state (issue #5): tracks a command's round-trip through the
// desktop command service, which retransmits a COMMAND_LONG until the flight
// controller acks it. The terminal outcome arrives on the `"command"` event.

import { useEffect, useState } from 'react'
import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { logEvent } from './eventlog'
import { useTelemetryStore } from './telemetry'
import { useTerrainStore } from './terrain'

/** Ground (AMSL) under a point, for the commands that must name a terrain
 * target: the mesh/DEM sample when terrain is loaded, else the vehicle's own
 * altitude (a vehicle on the ground knows its height). */
async function groundAmslAt(
  latDeg: number,
  lonDeg: number,
  fallbackAmslM: number,
): Promise<number> {
  try {
    const ground = await useTerrainStore.getState().samplePointAmsl(latDeg, lonDeg)
    if (ground != null && Number.isFinite(ground)) return ground
  } catch {
    /* no terrain: fall back to the vehicle's altitude */
  }
  return fallbackAmslM
}

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
  /** Wall-clock time the command left the UI, or `null` when nothing is in
   * flight. The status strip measures the round trip from here (plan §7 U4). */
  sentAtMs: number | null
  /** Round trip of the last terminal ack, in ms (frozen at the ack). */
  lastElapsedMs: number | null
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
  /** Drop the last result from the status strip once it is acknowledged (L2:
   * a rejection is kept on screen until the operator confirms it). */
  dismissResult: () => void
  reset: () => void
}

/** Set-home request awaiting its ack; promoted to `homeMarker` on accept. */
let pendingHome: [number, number, number] | null = null

/** Terminal bookkeeping: stop the clock and freeze the round trip. */
function settled(sentAtMs: number | null): {
  sentAtMs: null
  lastElapsedMs: number | null
} {
  return {
    sentAtMs: null,
    lastElapsedMs: sentAtMs === null ? null : Date.now() - sentAtMs,
  }
}

export const useCommandStore = create<CommandState>((set) => ({
  pending: null,
  lastEvent: null,
  sentAtMs: null,
  lastElapsedMs: null,
  gotoTarget: null,
  homeMarker: null,
  clearGotoTarget: () => set({ gotoTarget: null }),

  send: async (name) => {
    set({ pending: name, lastEvent: null, sentAtMs: null, lastElapsedMs: null })
    // Landing: name the ground under the vehicle so the FC lands on the
    // terrain instead of a HOME plane (operator report: the aircraft held
    // altitude over lower ground at the end of a landing). Takeoff does the
    // same with its AMSL climb target.
    const fix = useTelemetryStore.getState().snapshot?.global_position
    const ground =
      name === 'land' && fix
        ? await groundAmslAt(fix.latitude_deg, fix.longitude_deg, fix.altitude.meters)
        : null
    set({ sentAtMs: Date.now() })
    try {
      await invoke('send_command', { name, groundAmslM: ground })
    } catch (err) {
      set((s) => ({
        pending: null,
        ...settled(s.sentAtMs),
        lastEvent: {
          command: name,
          kind: 'failed',
          result: null,
          message: String(err),
        },
      }))
    }
  },

  sendSetMode: async (mainMode, subMode) => {
    set({ pending: 'set_mode', lastEvent: null, sentAtMs: Date.now(), lastElapsedMs: null })
    try {
      await invoke('send_set_mode', { mainMode, subMode })
    } catch (err) {
      set((s) => ({
        pending: null,
        ...settled(s.sentAtMs),
        lastEvent: {
          command: 'set_mode',
          kind: 'failed',
          result: null,
          message: String(err),
        },
      }))
    }
  },

  sendArmDisarm: async (arm) => {
    set({ pending: 'arm_disarm', lastEvent: null, sentAtMs: Date.now(), lastElapsedMs: null })
    try {
      await invoke('send_arm_disarm', { arm })
    } catch (err) {
      set((s) => ({
        pending: null,
        ...settled(s.sentAtMs),
        lastEvent: {
          command: 'arm_disarm',
          kind: 'failed',
          result: null,
          message: String(err),
        },
      }))
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
    // The terrain height under the *clicked* point, not the vehicle's own
    // altitude: home is usually re-set while airborne over the next site, and
    // an airborne altitude bakes a home plane far above the ground (every
    // HOME-referenced height, the landing target included, then sits too
    // high). Falls back to the current fix when no terrain is loaded.
    const fix = useTelemetryStore.getState().snapshot?.global_position
    const alt = await groundAmslAt(latDeg, lonDeg, fix?.altitude.meters ?? 0)
    pendingHome = [latDeg, lonDeg, alt]
    try {
      await invoke('send_set_home', { latDeg, lonDeg, amslM: alt })
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
    // Every terminal event freezes the round trip for the status strip.
    set((s) =>
      e.kind === 'sent'
        ? { lastEvent: e, pending: s.pending }
        : { lastEvent: e, pending: null, ...settled(s.sentAtMs) },
    )
  },

  dismissResult: () => {
    set({ lastEvent: null, lastElapsedMs: null })
  },

  reset: () => {
    pendingHome = null
    set({
      pending: null,
      lastEvent: null,
      sentAtMs: null,
      lastElapsedMs: null,
      gotoTarget: null,
      homeMarker: null,
    })
  },
}))

/** How often the in-flight elapsed time is refreshed (plan §7 U4). */
const PROGRESS_TICK_MS = 200

/** Where a command sits in its round trip (plan §7 U4 status strip). */
export type CommandPhase = 'sending' | 'awaiting_ack' | 'done'

export interface CommandProgress {
  /** The command in flight, or the last one that finished; null before any. */
  command: string | null
  phase: CommandPhase | null
  /** Milliseconds since the command left the UI — live while in flight. */
  elapsedMs: number | null
  /** The event the phase text is derived from. */
  result: CommandEventPayload | null
}

/**
 * Live view of the command round trip behind the status strip: "sent →
 * awaiting ACK → result + elapsed" (plan §7 U4). While a command is pending
 * the elapsed time ticks; once it ends the value frozen at the ack is shown,
 * so the strip never drifts away from when the FC answered.
 */
export function useCommandProgress(): CommandProgress {
  const pending = useCommandStore((s) => s.pending)
  const sentAtMs = useCommandStore((s) => s.sentAtMs)
  const lastEvent = useCommandStore((s) => s.lastEvent)
  const lastElapsedMs = useCommandStore((s) => s.lastElapsedMs)
  const [now, setNow] = useState(() => Date.now())

  useEffect(() => {
    if (pending === null) return
    setNow(Date.now())
    const id = window.setInterval(() => setNow(Date.now()), PROGRESS_TICK_MS)
    return () => window.clearInterval(id)
  }, [pending, sentAtMs])

  if (pending !== null) {
    return {
      command: pending,
      phase: lastEvent?.kind === 'sent' ? 'awaiting_ack' : 'sending',
      elapsedMs: sentAtMs === null ? null : now - sentAtMs,
      result: lastEvent,
    }
  }
  if (lastEvent === null) {
    return { command: null, phase: null, elapsedMs: null, result: null }
  }
  return {
    command: lastEvent.command,
    phase: 'done',
    elapsedMs: lastElapsedMs,
    result: lastEvent,
  }
}
