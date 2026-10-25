// Application-owned base state and independently versioned vehicle forwarding.
import { create } from 'zustand'
import type { RtkBaseMode as BaseMode } from '../generated-types/RtkBaseMode'
import type { BasePhase } from '../generated-types/BasePhase'
import type { BasePort } from '../generated-types/BasePort'
import type { BaseSessionId } from '../generated-types/BaseSessionId'
import type { BaseStatus } from '../generated-types/BaseStatus'
import type { ForeignState } from '../generated-types/ForeignState'
import type { RtkLinkStatus } from '../generated-types/RtkLinkStatus'
import type { RtkStats } from '../generated-types/RtkStats'
import type { SerialDeviceInfo } from '../generated-types/SerialDeviceInfo'
import type { SerialPortRef } from '../generated-types/SerialPortRef'
import { logEvent } from './eventlog'
import { useLinkStore } from './link'
import { useSerialAutoStore } from './serialAuto'

export { SURVEY_DEFAULTS, SURVEY_LIMITS } from '../util/rtkSettings'

export function baseRunning(phase: BasePhase | undefined): boolean {
  return phase != null && ['identifying', 'configuring', 'surveying', 'ready', 'stopping'].includes(phase)
}

function newerBase(next: BaseStatus, previous: BaseStatus | null): boolean {
  if (!previous) return true
  if (next.session_id === previous.session_id) return next.revision > previous.revision
  if (next.session_id == null) return false
  if (previous.session_id == null) return true
  return BigInt(next.session_id) > BigInt(previous.session_id)
}

function visibleStats(base: BaseStatus | null, link: RtkLinkStatus | null): RtkStats | null {
  const stats = link?.stats
  return base?.phase === 'ready' && stats?.base_session_id === base.session_id
    && useLinkStore.getState().link?.link_state === 'connected' ? stats ?? null : null
}

interface RtkState {
  base: BaseStatus | null
  baseReceivedAtMs: number | null
  linkSnapshot: RtkLinkStatus | null
  stats: RtkStats | null
  foreign: ForeignState | null
  ports: BasePort[]
  portsError: string | null
  busy: 'starting' | 'stopping' | null
  handleBase: (status: BaseStatus) => void
  handleLink: (status: RtkLinkStatus) => void
  resetLink: () => void
  loadPorts: () => Promise<void>
  startSerial: (device: SerialDeviceInfo, baudRate: number, mode: BaseMode, expected?: SerialPortRef | null) => Promise<string | null>
  stop: () => Promise<string | null>
}

let operation = 0
let portRequest = 0
let linkRevision = -1
let linkReset = 0

export const useRtkStore = create<RtkState>((set, get) => ({
  base: null, baseReceivedAtMs: null, linkSnapshot: null, stats: null, foreign: null,
  ports: [], portsError: null, busy: null,

  handleBase: (base) => {
    if (!newerBase(base, get().base)) return
    const previous = get().base
    if (base.phase !== previous?.phase || base.session_id !== previous?.session_id) {
      logEvent('rtk', base.phase === 'failed' ? 'warn' : 'info',
        `Base ${base.source ?? 'receiver'}: ${base.phase}${base.error ? ` — ${base.error}` : ''}`)
    }
    set({ base, baseReceivedAtMs: Date.now(), stats: visibleStats(base, get().linkSnapshot) })
  },

  handleLink: (linkSnapshot) => {
    if (linkSnapshot.revision <= linkRevision) return
    linkRevision = linkSnapshot.revision
    const stats = visibleStats(get().base, linkSnapshot)
    const previous = get().stats
    if (stats?.stalled && !previous?.stalled) logEvent('rtk', 'warn', 'Base source data is stale')
    if (stats?.error && stats.error !== previous?.error) logEvent('rtk', 'warn', stats.error)
    if (linkSnapshot.foreign.active && !get().foreign?.active) {
      logEvent('rtk', 'warn', 'Another RTCM injector detected')
    }
    set({ linkSnapshot, stats, foreign: useLinkStore.getState().link?.link_state === 'connected' ? linkSnapshot.foreign : null })
  },

  resetLink: () => {
    linkReset += 1
    set({ linkSnapshot: null, stats: null, foreign: null })
  },

  loadPorts: async () => {
    const request = ++portRequest
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const ports = await invoke<BasePort[]>('rtk_serial_ports')
      if (request === portRequest) set({ ports: Array.isArray(ports) ? ports : [], portsError: null })
    } catch (error) {
      if (request === portRequest) set({ ports: [], portsError: String(error) })
    }
  },

  startSerial: async (device, baudRate, mode, expected) => {
    if (get().busy || baseRunning(get().base?.phase)) return 'A base operation is already active'
    const request = ++operation
    set({ busy: 'starting' })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const port = expected ?? get().ports.find((p) => p.device.port_name === device.port_name)?.port_ref ?? undefined
      const id = await invoke<BaseSessionId>('rtk_start_serial', { device, port, baudRate, mode })
      if (request !== operation) return null
      // An event can precede the command reply. Never overwrite its real
      // configuration/survey status with an optimistic starting snapshot.
      get().handleBase({
        session_id: id, revision: 0, phase: 'identifying', source: device.port_name,
        serial: { port_name: device.port_name, baud_rate: baudRate }, mode,
        receiver: null, survey_in: null, seconds_since_data: null, seconds_since_rtcm: null, error: null, failure_kind: null,
      })
      return null
    } catch (error) {
      return request === operation ? String(error) : null
    } finally {
      if (request === operation) set({ busy: null })
    }
  },

  stop: async () => {
    const sessionId = get().base?.session_id
    if (!sessionId || get().busy === 'stopping') return null
    const request = ++operation
    set({ busy: 'stopping' })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const expectedIntent = useSerialAutoStore.getState().snapshot?.rtk.intent
      await invoke('rtk_stop', { sessionId, expectedIntent })
      // The native event may be queued behind the reply; hydrate its actual
      // revision rather than inventing a local completion or stopping a new ID.
      const base = await invoke<BaseStatus>('rtk_base_status')
      if (base) get().handleBase(base)
      return null
    } catch (error) {
      return get().base?.session_id === sessionId ? String(error) : null
    } finally {
      if (request === operation) set({ busy: null })
    }
  },
}))

let watcherStarted = false
export function startRtkWatcher(): void {
  if (watcherStarted) return
  watcherStarted = true
  void (async () => {
    const unlisten: (() => void)[] = []
    try {
      const { listen } = await import('@tauri-apps/api/event')
      const { invoke } = await import('@tauri-apps/api/core')
      unlisten.push(await listen<BaseStatus>('rtk_base_status', (event) => useRtkStore.getState().handleBase(event.payload)))
      unlisten.push(await listen<RtkLinkStatus>('rtk_link_status', (event) => useRtkStore.getState().handleLink(event.payload)))
      const reset = linkReset
      const [base, link] = await Promise.all([
        invoke<BaseStatus>('rtk_base_status'), invoke<RtkLinkStatus>('rtk_link_status'),
      ])
      if (base) useRtkStore.getState().handleBase(base)
      if (link && reset === linkReset) useRtkStore.getState().handleLink(link)
    } catch {
      unlisten.forEach((stop) => stop())
      watcherStarted = false
    }
  })()
}
