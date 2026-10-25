import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import type { AutoConnectRole } from '../generated-types/AutoConnectRole'
import type { SerialAutoConnectSettings } from '../generated-types/SerialAutoConnectSettings'
import type { SerialAutoConnectSnapshot } from '../generated-types/SerialAutoConnectSnapshot'
import { legacyRtkSettings } from '../util/rtkSettings'

interface SerialAutoState {
  snapshot: SerialAutoConnectSnapshot | null
  busy: boolean
  error: string | null
  handle: (snapshot: SerialAutoConnectSnapshot | null) => void
  save: (settings: SerialAutoConnectSettings) => Promise<void>
  pause: (role: AutoConnectRole) => Promise<void>
  resume: (role: AutoConnectRole) => Promise<void>
}

function message(error: unknown): string {
  return typeof error === 'object' && error !== null && 'message' in error
    ? String(error.message) : String(error)
}

export const useSerialAutoStore = create<SerialAutoState>((set, get) => ({
  snapshot: null, busy: false, error: null,
  handle: (snapshot) => {
    if (!snapshot || typeof snapshot.revision !== 'string' || !snapshot.settings || !snapshot.fc || !snapshot.rtk) return
    const current = get().snapshot
    try { if (current && BigInt(snapshot.revision) <= BigInt(current.revision)) return } catch { return }
    set({ snapshot })
  },
  save: async (settings) => {
    const current = get().snapshot
    if (!current || get().busy) return
    set({ busy: true, error: null })
    try {
      get().handle(await invoke<SerialAutoConnectSnapshot>('serial_auto_connect_save', {
        expected: current.settings_revision, settings,
      }))
    } catch (error) { set({ error: message(error) }) }
    finally { set({ busy: false }) }
  },
  pause: async (role) => {
    const current = get().snapshot
    if (!current || get().busy) return
    set({ busy: true, error: null })
    try {
      await invoke('serial_auto_connect_pause', { expected: role === 'primary_fc' ? current.fc.intent : current.rtk.intent })
      get().handle(await invoke<SerialAutoConnectSnapshot>('serial_auto_connect_status'))
    } catch (error) { set({ error: message(error) }) }
    finally { set({ busy: false }) }
  },
  resume: async (role) => {
    const current = get().snapshot
    if (!current || get().busy) return
    set({ busy: true, error: null })
    try {
      await invoke('serial_auto_connect_resume', { expected: role === 'primary_fc' ? current.fc.intent : current.rtk.intent })
      get().handle(await invoke<SerialAutoConnectSnapshot>('serial_auto_connect_status'))
    } catch (error) { set({ error: message(error) }) }
    finally { set({ busy: false }) }
  },
}))

let started = false
export function startSerialAutoWatcher(): void {
  if (started) return
  started = true
  void (async () => {
    let stop: (() => void) | undefined
    try {
      const { listen } = await import('@tauri-apps/api/event')
      stop = await listen<SerialAutoConnectSnapshot>('serial_auto_connect_status', ({ payload }) => useSerialAutoStore.getState().handle(payload))
      useSerialAutoStore.getState().handle(await invoke<SerialAutoConnectSnapshot | null>('serial_auto_connect_status', { legacyRtk: legacyRtkSettings() }))
    } catch {
      stop?.()
      started = false
    }
  })()
}

if (import.meta.env?.DEV && typeof window !== 'undefined') {
  ;(window as unknown as { __mgSerialAuto?: unknown }).__mgSerialAuto = useSerialAutoStore
}
