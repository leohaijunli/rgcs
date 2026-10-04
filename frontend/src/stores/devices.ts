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
      const devices = await invoke<SerialDeviceInfo[]>('enumerate_devices')
      set({ devices, loaded: true })
    } catch {
      // Browser dev (no Tauri runtime): report empty.
      set({ devices: [], loaded: true })
    }
  },
}))