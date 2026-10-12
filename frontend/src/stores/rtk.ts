// RTK injection state (R4): the 1 Hz stats pushed by the shell onto the
// `rtk_stats` event, plus the drawer's start/stop actions. The rover fix
// type itself stays in the telemetry store (the drawer reads it there).

import { create } from 'zustand'
import type { RtkStats } from '../generated-types/RtkStats'
import type { ForeignState } from '../generated-types/ForeignState'
import { logEvent } from './eventlog'

interface RtkState {
  /** Latest 1 Hz stats; null = no active injection source (or not connected). */
  stats: RtkStats | null
  /** Foreign-injector snapshot (N4): another GCS sending RTCM at us. */
  foreign: ForeignState | null
  /** Start/stop request in flight. */
  busy: boolean
  handleEvent: (s: RtkStats) => void
  handleForeign: (f: ForeignState) => void
  startSerial: (portName: string, baudRate: number) => Promise<string | null>
  stop: () => Promise<void>
}

export const useRtkStore = create<RtkState>((set) => ({
  stats: null,
  foreign: null,
  busy: false,

  handleEvent: (stats) => {
    // Log the state transitions an operator should not miss (stall, drops).
    set((s) => {
      const prev = s.stats
      if (stats.stalled && !prev?.stalled) {
        logEvent('rtk', 'warn', `RTCM injection stalled (${stats.source ?? 'no source'})`)
      }
      if (!prev?.dropped_chunks && stats.dropped_chunks > 0) {
        logEvent('rtk', 'warn', `RTCM chunks dropped: ${stats.dropped_chunks}`)
      }
      if (prev?.source !== stats.source) {
        logEvent(
          'rtk',
          'info',
          stats.source ? `Injection source: ${stats.source}` : 'Injection stopped',
        )
      }
      return { stats }
    })
  },

  handleForeign: (f) => {
    // Log the transitions an operator should not miss (N4/ADR-008).
    set((s) => {
      if (f.active && !s.foreign?.active) {
        logEvent('rtk', 'warn', `Another RTCM injector detected (${f.packets} packets)`)
      }
      if (!f.active && s.foreign?.active) {
        logEvent('rtk', 'info', 'Foreign RTCM injector silent again')
      }
      return { foreign: f }
    })
  },

  startSerial: async (portName, baudRate) => {
    set({ busy: true })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('rtk_start_serial', { portName, baudRate })
      return null
    } catch (e) {
      return String(e)
    } finally {
      set({ busy: false })
    }
  },

  stop: async () => {
    set({ busy: true })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('rtk_stop')
    } catch {
      /* not connected */
    } finally {
      set({ busy: false })
    }
  },
}))

/** Subscribe once at drawer mount (desktop only). */
let watcherStarted = false
export function startRtkWatcher(): void {
  if (watcherStarted) return
  watcherStarted = true
  void (async () => {
    try {
      const { listen } = await import('@tauri-apps/api/event')
      const { invoke } = await import('@tauri-apps/api/core')
      await listen<RtkStats>('rtk_stats', (e) =>
        useRtkStore.getState().handleEvent(e.payload),
      )
      await listen<ForeignState>('rtk_foreign', (e) =>
        useRtkStore.getState().handleForeign(e.payload),
      )
      // Seed from the cached snapshot (events only fire on change).
      const seed = await invoke<ForeignState | null>('rtk_foreign_state')
      if (seed) useRtkStore.getState().handleForeign(seed)
    } catch {
      /* browser dev: no shell events */
    }
  })()
}
