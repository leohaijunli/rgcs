// Browser-mode mock data source: feeds a fixed signal set when the inspector
// runs outside Tauri (the plan's mock, mirroring `telemetry/mock`).

import type { SignalSample } from '../generated-types/SignalSample'

export interface MockSignal {
  id: string
  signal: { system_id: number; component_id: number; message_id: number; field: string }
  base: number
  amp: number
  freqHz: number
}

const SIGS: MockSignal[] = [
  { id: 'roll', signal: { system_id: 1, component_id: 1, message_id: 30, field: 'roll' }, base: 0.02, amp: 0.4, freqHz: 0.1 },
  { id: 'pitch', signal: { system_id: 1, component_id: 1, message_id: 30, field: 'pitch' }, base: 0.01, amp: 0.3, freqHz: 0.08 },
  { id: 'yaw', signal: { system_id: 1, component_id: 1, message_id: 30, field: 'yaw' }, base: 1.5, amp: 0.2, freqHz: 0.05 },
  { id: 'xmag', signal: { system_id: 1, component_id: 1, message_id: 105, field: 'xmag' }, base: 22000, amp: 120, freqHz: 0.4 },
  { id: 'ymag', signal: { system_id: 1, component_id: 1, message_id: 105, field: 'ymag' }, base: -3000, amp: 80, freqHz: 0.6 },
  { id: 'zmag', signal: { system_id: 1, component_id: 1, message_id: 105, field: 'zmag' }, base: 42000, amp: 90, freqHz: 0.3 },
]

/** PX4/MAVLink message names for the signal browser group headers. Only the
 * ids PX4 commonly streams are listed; anything else falls back to `msg <id>`.
 * S2 will replace this map with `message_name` from the Rust catalog. */
const MESSAGE_NAMES: Record<number, string> = {
  0: 'HEARTBEAT',
  1: 'SYS_STATUS',
  22: 'NAMED_VALUE_FLOAT',
  24: 'GPS_RAW_INT',
  30: 'ATTITUDE',
  32: 'LOCAL_POSITION_NED',
  33: 'GLOBAL_POSITION_INT',
  65: 'RC_CHANNELS',
  74: 'VFR_HUD',
  105: 'HIGHRES_IMU',
  129: 'ALTITUDE',
  132: 'ESTIMATOR_STATUS',
  147: 'BATTERY_STATUS',
  250: 'DEBUG_VECT',
  251: 'DEBUG_FLOAT',
  254: 'DEBUG',
}

/** Display name for a message id (group header in the signal browser). */
export function msgName(messageId: number): string {
  return MESSAGE_NAMES[messageId] ?? `msg ${messageId}`
}

export function mockCatalog(): { signal: MockSignal['signal']; last_value: number; rate_hz: number; last_seen_ms: number }[] {
  return SIGS.map((s) => ({ signal: s.signal, last_value: s.base, rate_hz: 100, last_seen_ms: 0 }))
}

/** Signal id string for the mock (the frontend keys its buffers by it). */
export function signalKey(s: MockSignal['signal']): string {
  return `${MESSAGE_NAMES[s.message_id] ?? s.message_id}.${s.field}`
}

/** One mock sample batch, at 100 Hz. */
export function mockSamples(t: number): SignalSample[] {
  return SIGS.map((s) => {
    const value = s.base + s.amp * Math.sin(2 * Math.PI * s.freqHz * t) + (Math.random() - 0.5) * s.amp * 0.1
    return {
      id: { system_id: s.signal.system_id, component_id: s.signal.component_id, message_id: s.signal.message_id, field: s.signal.field },
      t_ms: t * 1000,
      value,
    }
  })
}

/** Whether Tauri is available (mock mode otherwise). */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}