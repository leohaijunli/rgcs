// Browser-mode mock data source: feeds a fixed signal set when the inspector
// runs outside Tauri (the plan's mock, mirroring `telemetry/mock`).

import type { SignalSample } from '../generated-types/SignalSample'
import type { AlgorithmInfo, ParamSpec } from './dsp'

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

export function mockCatalog(): {
  signal: MockSignal['signal']
  message_name: string
  last_value: number
  rate_hz: number
  last_seen_ms: number
}[] {
  return SIGS.map((s) => ({
    signal: s.signal,
    message_name: msgName(s.signal.message_id),
    last_value: s.base,
    rate_hz: 100,
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

const float = (key: string, label: string, def: number, min: number, max: number, step: number): ParamSpec => ({
  key,
  label,
  unit: null,
  kind: { kind: 'Float', min, max, step, log: false },
  default: def,
})
const int = (key: string, label: string, def: number, min: number, max: number): ParamSpec => ({
  key,
  label,
  unit: null,
  kind: { kind: 'Int', min, max },
  default: def,
})
const enumeration = (key: string, label: string, def: number, options: string[]): ParamSpec => ({
  key,
  label,
  unit: null,
  kind: { kind: 'Enum', options },
  default: def,
})
const bool = (key: string, label: string, def: number): ParamSpec => ({
  key,
  label,
  unit: null,
  kind: { kind: 'Bool' },
  default: def,
})

/** The registry mirror for mock mode (`core::dsp::registry` is the source of
 * truth); keeps the filter UI live without Tauri. */
export function mockAlgorithms(): AlgorithmInfo[] {
  return [
    {
      id: 'lpf2',
      name: '2nd-order low-pass',
      kind: 'processor',
      params: [float('fc_hz', 'Cutoff', 5, 0.001, 1000, 0.1), float('q', 'Q', 0.707, 0.001, 100, 0.01)],
    },
    {
      id: 'hpf2',
      name: '2nd-order high-pass',
      kind: 'processor',
      params: [float('fc_hz', 'Cutoff', 5, 0.001, 1000, 0.1), float('q', 'Q', 0.707, 0.001, 100, 0.01)],
    },
    { id: 'moving_average', name: 'Moving average', kind: 'processor', params: [int('window', 'Window', 10, 1, 1000)] },
    {
      id: 'detrend',
      name: 'Detrend',
      kind: 'processor',
      params: [int('window', 'Window', 100, 2, 10000), bool('mode', 'Linear', 1)],
    },
    {
      id: 'fft',
      name: 'Realtime FFT',
      kind: 'analyzer',
      params: [
        int('n', 'FFT length', 1024, 16, 8192),
        enumeration('window', 'Window', 1, ['rectangular', 'hann', 'hamming', 'blackman', 'flat_top']),
        enumeration('scale', 'Scale', 0, ['magnitude', 'psd', 'decibels']),
        bool('detrend', 'Detrend', 0),
      ],
    },
  ]
}
