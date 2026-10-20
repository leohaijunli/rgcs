// Browser-mode mock data source: feeds a fixed signal set when the inspector
// runs outside Tauri (the plan's mock, mirroring `telemetry/mock`).
//
// The mock shows raw curves only: filtering and the FFT analyzer run in Rust
// (`core::inspector::session`), so in a plain browser there is no DSP runtime
// and the filter/FFT entry points are hidden rather than faked.

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

/** PX4/MAVLink message names for the mock catalog. In Tauri mode the name
 * comes from the Rust catalog (`CatalogEntry.message_name`); this map only
 * feeds the browser-mode mock. */
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

/** Display name for a message id (mock fallback). */
export function msgName(messageId: number): string {
  return MESSAGE_NAMES[messageId] ?? `msg ${messageId}`
}

/** Monotonic counter for deterministic fault rotation across batches. */
let tick = 0

/** Fault injection is opt-in via `?fault` so the normal mock stays clean while
 * the Playwright smoke test can force NaN/null like the real link would. */
function faultsEnabled(): boolean {
  return typeof location !== 'undefined' && new URLSearchParams(location.search).has('fault')
}

export function mockCatalog(): {
  signal: MockSignal['signal']
  message_name: string
  last_value: number
  rate_hz: number
  last_seen_ms: number
}[] {
  const fault = faultsEnabled()
  return SIGS.map((s, i) => ({
    signal: s.signal,
    message_name: msgName(s.signal.message_id),
    // `?fault` reproduces the real Tauri path, where serde_json turns a NaN
    // field into `null`: the tree formatter must render `—`, not throw.
    last_value: fault && i === 0 ? (null as unknown as number) : s.base,
    rate_hz: fault && i === 1 ? (null as unknown as number) : 100,
    last_seen_ms: 0,
  }))
}

/** Stable frontend key for a signal: `sys:comp:msguid.field` (S2). Includes
 * the node ids so multi-vehicle setups do not collide. */
export function signalKey(s: MockSignal['signal']): string {
  return `${s.system_id}:${s.component_id}:${s.message_id}.${s.field}`
}

/** One mock sample batch, at 100 Hz. */
export function mockSamples(t: number): SignalSample[] {
  const fault = faultsEnabled()
  tick += 1
  return SIGS.map((s, i) => {
    let value = s.base + s.amp * Math.sin(2 * Math.PI * s.freqHz * t) + (Math.random() - 0.5) * s.amp * 0.1
    // `?fault`: drop one rotating signal to NaN once a second, stressing the
    // uPlot gap path (plan S5).
    if (fault && tick % 100 === 0 && i === tick / 100 % SIGS.length) value = NaN
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
