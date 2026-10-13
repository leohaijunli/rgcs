// MAVLink console (QGC-style, see `console_service.rs`): sends PX4 NuttShell
// commands over `SERIAL_CONTROL` (device SHELL) and renders the reply frames
// the shell echoes back as one scrolling transcript. Preset buttons offer
// the commands that matter for a survey flight.

import { create } from 'zustand'
import { listen } from '@tauri-apps/api/event'

/** Keep at most this many transcript lines (ring). */
const MAX_LINES = 1000

/** PX4 NuttShell commands useful for a survey flight / motor test. */
export const CONSOLE_PRESETS: string[] = [
  'help',
  'ver',
  'top',
  'dmesg',
  'free',
  'ps',
  'listener sensor_combined',
  'listener vehicle_gps_position',
  'param show',
  'gps status',
]

interface ConsoleState {
  /** Transcript lines (output reassembled from reply chunks). */
  lines: string[]
  /** True while a command is being sent. */
  busy: boolean
  send: (command: string) => Promise<void>
  close: () => Promise<void>
  startWatcher: () => void
}

let started = false

export const useConsoleStore = create<ConsoleState>((set, get) => ({
  lines: [],
  busy: false,
  send: async (command) => {
    if (!command.trim() || get().busy) return
    set({ busy: true })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('console_send', { command })
    } catch (e) {
      set({ busy: false })
      throw e
    }
    set({ busy: false })
  },
  close: async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('console_close')
    } catch {
      /* no link */
    }
  },
  startWatcher: () => {
    if (started) return
    started = true
    const appendChunk = (chunk: string) => {
      const normalized = chunk.replace(/\r/g, '')
      const parts = normalized.split('\n')
      const lines = [...get().lines]
      if (lines.length === 0) lines.push('')
      // First part continues the current line; the rest become new lines.
      lines[lines.length - 1] += parts[0]
      for (let i = 1; i < parts.length; i++) lines.push(parts[i])
      // Drop a trailing empty line left by a chunk ending in '\n'.
      while (lines.length > 1 && lines[lines.length - 1] === '') lines.pop()
      if (lines.length > MAX_LINES) lines.splice(0, lines.length - MAX_LINES)
      set({ lines })
    }
    void (async () => {
      try {
        await listen<string>('console_output', (e) => appendChunk(e.payload))
      } catch {
        /* events unavailable (browser dev) */
      }
    })()
  },
}))