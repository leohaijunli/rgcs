// Serial device store (issues.md #40/#41): enumeration plus a hotplug
// subscription so the connection dropdown stays live without a manual
// refresh.

import { create } from 'zustand'
import type { SerialDeviceInfo } from '../generated-types/SerialDeviceInfo'

interface DevicesState {
  devices: SerialDeviceInfo[]
  loaded: boolean
  load: () => Promise<void>
}

export const useDevicesStore = create<DevicesState>((set) => ({
  devices: [],
  loaded: false,
  load: async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      // A shell that answers `null` (a failed scan, an older build) must not
      // leave `devices` null: every consumer reads `.length` (RtkDrawer's
      // effect blanks the whole window otherwise — found 2026-10-21).
      const devices = await invoke<SerialDeviceInfo[] | null>('enumerate_devices')
      set({ devices: Array.isArray(devices) ? devices : [], loaded: true })
    } catch {
      // Browser dev (no Tauri runtime): report empty.
      set({ devices: [], loaded: true })
    }
  },
}))

let watcherStarted = false

/**
 * Serial hotplug subscription (issues.md #40): the shell pushes a fresh
 * device list whenever a port appears or disappears. Idempotent — call once
 * at app startup (desktop only).
 */
export function startDeviceWatcher(): void {
  if (watcherStarted) return
  watcherStarted = true
  void (async () => {
    try {
      const { listen } = await import('@tauri-apps/api/event')
      await listen<SerialDeviceInfo[]>('devices_changed', (e) => {
        useDevicesStore.setState({
          devices: Array.isArray(e.payload) ? e.payload : [],
          loaded: true,
        })
      })
    } catch {
      /* browser dev: no events */
    }
  })()
}