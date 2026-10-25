// Serial device store (issues.md #40/#41): enumeration plus a hotplug
// subscription so the connection dropdown stays live without a manual
// refresh.

import { create } from 'zustand'
import type { SerialDeviceInfo } from '../generated-types/SerialDeviceInfo'
import type { SerialInventory } from '../generated-types/SerialInventory'
import type { SerialPortRef } from '../generated-types/SerialPortRef'
import { parseEndpoint } from '../desktop/endpoint'

interface DevicesState {
  devices: SerialDeviceInfo[]
  loaded: boolean
  inventory: SerialInventory | null
  error: string | null
  handleInventory: (inventory: SerialInventory | null) => void
  load: () => Promise<void>
}

export const useDevicesStore = create<DevicesState>((set) => ({
  devices: [],
  loaded: false,
  inventory: null,
  error: null,
  handleInventory: (inventory) => {
    if (!inventory || typeof inventory.revision !== 'string' || !Array.isArray(inventory.ports) || !inventory.health) return
    set((current) => current.inventory && BigInt(inventory.revision) <= BigInt(current.inventory.revision)
      ? current : { inventory, devices: inventory.ports.map((p) => p.port.device), loaded: true,
        error: inventory.health.kind === 'unavailable' ? inventory.health.message : null })
  },
  load: async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const inventory = await invoke<SerialInventory | null>('serial_inventory')
      if (inventory) {
        useDevicesStore.getState().handleInventory(inventory)
        return
      }
      // A shell that answers `null` (a failed scan, an older build) must not
      // leave `devices` null: every consumer reads `.length` (RtkDrawer's
      // effect blanks the whole window otherwise — found 2026-10-21).
      const devices = await invoke<SerialDeviceInfo[] | null>('enumerate_devices')
      set({ devices: Array.isArray(devices) ? devices : [], loaded: true })
    } catch (error) {
      // Browser dev (no Tauri runtime): report empty.
      set({ devices: [], loaded: true, error: String(error) })
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
      await listen<SerialInventory>('serial_inventory', (e) => useDevicesStore.getState().handleInventory(e.payload))
      await listen<SerialDeviceInfo[]>('devices_changed', (e) => {
        if (useDevicesStore.getState().inventory) return
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

/** Capture the attachment shown in the current inventory for a manual request. */
export async function serialPortForEndpoint(endpoint: string): Promise<SerialPortRef | null | undefined> {
  const draft = parseEndpoint(endpoint)
  if (draft?.kind !== 'serial') return null
  startDeviceWatcher()
  if (!useDevicesStore.getState().loaded) await useDevicesStore.getState().load()
  const { inventory, error } = useDevicesStore.getState()
  // Older shells/browser fixtures have the legacy list only. Current native
  // commands require a real attachment reference and reject a missing one.
  if (!inventory) {
    if (error) throw new Error(error)
    return undefined
  }
  if (inventory.health.kind !== 'ready') throw new Error('Serial device inventory is not ready')
  const selected = inventory.ports.find((p) => p.port.device.port_name === draft.host)
  if (selected) return selected.port
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<SerialPortRef>('serial_resolve_endpoint', { endpoint })
}
