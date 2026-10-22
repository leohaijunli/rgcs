// Motor / mag interference test window — A1+A3 (motor-test §5–§6, §8.2),
// refactored to a source-driven layout (operator request 2026-10-10):
//
//   Header: live chips + state banner + EMERGENCY STOP
//   Left (one workflow): prop load → command source → source panel → safety
//   Mid: compact state strip + reserved space (GoPro video, curves)
//   Right: live status
//
// One primary action per state: START (hold for full props) dispatches by
// source — manual sliders or a preset waveform; STOP while running. The
// safety gate (unlock → [typed phrase] → hold) applies to every source.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { readTextFile } from '@tauri-apps/plugin-fs'
import type { ParamEntry } from '../generated-types/ParamEntry'
import type { LogEntryInfo } from '../generated-types/LogEntryInfo'
import { isTauri } from '../inspector/mock'
import SignalScope, { useScopeFeed } from './SignalScope'
import StartStopButton from './StartStopButton'
import { scopeStore } from './scopeStore'

/** The parameter that gates motor tests on the FC (parameters plan PM9). */
const MOT_TEST_EN = 'COM_MOT_TEST_EN'
/** Poll period for the parameter cache check. */
const MOT_TEST_EN_POLL_MS = 2000

/** Prop load selected for this test. Operator-declared — there is no
 * sensor that can tell the types apart (operator decision 2026-10-10). */
type Load = 'none' | 'plate' | 'props'

const LOADS: Array<{ id: Load; label: string }> = [
  { id: 'none', label: 'No props' },
  { id: 'plate', label: 'Flat plate' },
  { id: 'props', label: 'Full props' },
]

/** PX4 v1.17 SDLOG_MODE meanings shown read-only in the capture panel. */
const SDLOG_MODE_LABELS: Record<number, string> = {
  0: 'armed→disarm',
  1: 'boot→disarm',
  2: 'boot→shutdown',
  3: 'AUX1 switch',
  4: '1st arm→shutdown',
}

function sdlogModeLabel(value: number | null): string {
  if (value == null) return 'unknown'
  return SDLOG_MODE_LABELS[value] ?? `unknown (${value})`
}

/** PX4 SDLOG_BACKEND is a bitmask: 1 = SD card, 2 = MAVLink. */
function sdlogBackendLabel(value: number | null): string {
  if (value == null) return 'unknown'
  const parts = [value & 1 ? 'SD card' : null, value & 2 ? 'MAVLink' : null].filter(
    (part): part is string => part != null,
  )
  return parts.length > 0 ? `${parts.join(' + ')} (${value})` : `none (${value})`
}

function captureErrorLabel(error: SdCaptureError | null): string {
  if (!error) return ''
  const details = Object.entries(error)
    .filter(([key]) => key !== 'kind')
    .map(([key, value]) => `${key}=${String(value)}`)
  return [error.kind.replaceAll('_', ' '), ...details].join(': ')
}

/** Command sources. Manual = live sliders; Preset = bounded waveform
 * (self-stopping); Forward = realtime SITL→FC forwarding (design pending). */
type Source = 'manual' | 'preset' | 'forward'

/** Preset waveform kinds, mirrored from `core::motor_test::source`. */
type PresetKind = 'step' | 'ramp' | 'square' | 'sine'

const SOURCES: Array<{ id: Source; label: string; hint: string }> = [
  { id: 'manual', label: 'Manual', hint: 'Live sliders drive the motors' },
  { id: 'preset', label: 'Preset', hint: 'Bounded waveform, stops itself' },
  {
    id: 'forward',
    label: 'Forward',
    hint: 'Realtime SITL→FC forwarding (SERVO_OUTPUT_RAW 1–4)'
  },
]

const PRESETS: Array<{ id: PresetKind; label: string }> = [
  { id: 'step', label: 'Step' },
  { id: 'ramp', label: 'Ramp' },
  { id: 'square', label: 'Square' },
  { id: 'sine', label: 'Sine' },
]

/** The typed phrase for full props (a deliberate gesture, not a click). */
const PROPS_PHRASE = 'START PROPS'
/** Hold duration that arms Start under full props. */
const PROPS_HOLD_MS = 3000

/** Motor count (fixed 4 until a vehicle config lands, D9). */
const MOTOR_COUNT = 4

/** Slider updates are merged at ~30 Hz — never queued (plan §8.1). */
const SET_VALUES_MIN_INTERVAL_MS = 33

/** One Hz status poll; safety-critical signals (armed!) are low-rate. */
const STATUS_POLL_MS = 1000

interface ActuatorStatus {
  connected: boolean
  fc_alive: boolean
  armed: boolean
  /** Raw PX4 custom-mode bitmask; decoded for display via util/px4mode. */
  custom_mode: number
  /** PX4 mode pre-decoded in core (ADR-017). */
  mode_label: string
  autopilot_px4: boolean
  endpoint: string | null
  tick_hz: number
  session: string
  /** SITL source link (revised D3). */
  sitl_connected: boolean
  sitl_endpoint: string | null
  /** Node and PWM bank selected for the SITL source. */
  sitl_source: SitlSourceConfig | null
  sitl_feed_age_s: number | null
  /** The motor test's own target link (real PX4, serial). */
  target_connected: boolean
  /** The serial target PX4's heartbeat is fresh (not the primary link). */
  target_fc_alive: boolean
  target_endpoint: string | null
  target_system_id: number | null
  /** COM_MOT_TEST_EN read from the target PX4 (null = unknown). */
  com_mot_test_en: number | null
  /** SDLOG_MODE read from the target PX4 (null = unknown), PX4 v1.17:
   * 0=armed until disarm, 1=boot until disarm, 2=boot until shutdown,
   * 3=AUX1 switch, 4=first arm until shutdown. */
  sdlog_mode: number | null
  /** SDLOG_BACKEND read from the target PX4 (null = unknown); bitmask:
   * 1=SD card, 2=MAVLink. */
  sdlog_backend: number | null
  /** Normalized SITL servo outputs 1–4 (the forward INPUT). */
  input_values: number[]
  /** The SITL's raw SERVO_OUTPUT_RAW 1–4 in µs (what input_values came from). */
  input_pwm: number[]
  /** Values sent to the FC as ACTUATOR_TEST (the OUTPUT), 0 when idle. */
  output_values: number[]
  /** Actual PWM for each motor function in µs, selected from its mapped
   * MAIN/AUX channel; null = unmapped or no fresh sample. */
  fc_output_pwm: (number | null)[]
  /** Motor function → physical output mapping read from PWM_*_FUNCx. */
  output_map: OutputMap
  /** Per-motor target PWM model used to compensate ACTUATOR_TEST and
   * forwarded SITL pulses (endpoints, reversal, THR_MDL_FAC, disabled). */
  pwm_models: PwmModel[]
  /** Target-shell SD-file capture state (Task 6). */
  sd_capture: SdCaptureStatus
  /** Per-test run coordinator + recovery manifest (Task 8). */
  run: RunSnapshot
}

type RunPhase =
  | 'starting_capture'
  | 'preroll'
  | 'motor_ready'
  | 'motor_running'
  | 'postroll'
  | 'stopping_capture'
  | 'complete'
  | 'aborted'
  | 'interrupted'
  | 'failed'

interface RunStatus {
  test_id: string
  phase: RunPhase
  remote_path: string | null
  stop_reason: string | null
  error: { kind: string; [key: string]: unknown } | null
}

interface RunSnapshot {
  status: RunStatus | null
  manifest_path: string | null
}

type CapturePhase =
  | 'idle'
  | 'verifying'
  | 'ready'
  | 'starting'
  | 'recording'
  | 'existing_recording'
  | 'stopping'
  | 'finalizing'
  | 'complete'
  | 'failed'

interface SdCaptureError {
  kind: string
  [key: string]: unknown
}

interface SdCaptureStatus {
  phase: CapturePhase
  test_id: string | null
  remote_path: string | null
  logger: {
    running: boolean
    backend: string
    file_logging: boolean
    remote_path: string | null
  } | null
  sdlog_mode: number | null
  sdlog_backend: number | null
  last_error: SdCaptureError | null
}

interface PwmModel {
  min_us: number
  max_us: number
  reversed: boolean
  thrust_factor: number
  disabled: boolean
}

type ServoPort = 'main' | 'aux'

interface ServoChannel {
  port: ServoPort
  channel: number
}

interface OutputAssignment {
  motor: number
  channel: ServoChannel | null
}

interface MappingIssue {
  kind: 'unreadable' | 'duplicate'
  port?: ServoPort
  channel?: number
  motor?: number
  first?: ServoChannel
  second?: ServoChannel
}

interface OutputMap {
  motors: OutputAssignment[]
  state: 'complete' | 'partial' | 'invalid' | 'unknown'
  issues: MappingIssue[]
}

interface SitlSourceConfig {
  endpoint: string
  system_id: number
  component_id: number
  port: ServoPort
}

function channelLabel(channel: ServoChannel): string {
  return `${channel.port === 'main' ? 'MAIN' : 'AUX'}${channel.channel}`
}

function assignmentFor(map: OutputMap, motor: number): OutputAssignment | undefined {
  return map.motors.find((assignment) => assignment.motor === motor)
}

interface SitlSourceFieldsProps {
  endpoint: string
  onEndpoint: (value: string) => void
  systemId: number
  onSystemId: (value: number) => void
  componentId: number
  onComponentId: (value: number) => void
  port: ServoPort
  onPort: (value: ServoPort) => void
  connected: boolean
  error: string | null
  onConnect: () => void
  onDisconnect: () => void
}

function SitlSourceFields(props: SitlSourceFieldsProps) {
  const validIds =
    Number.isInteger(props.systemId) &&
    props.systemId >= 1 &&
    props.systemId <= 255 &&
    Number.isInteger(props.componentId) &&
    props.componentId >= 1 &&
    props.componentId <= 255
  return (
    <>
      <input
        className="phrase-input"
        style={{ width: '100%', margin: 0 }}
        value={props.endpoint}
        onChange={(e) => props.onEndpoint(e.target.value)}
        placeholder="udpin:0.0.0.0:14540"
        aria-label="SITL endpoint"
        spellCheck={false}
        disabled={props.connected}
      />
      <label className="motor-row" style={{ marginTop: 6 }}>
        <span className="motor-label" style={{ width: 92 }}>System ID</span>
        <input
          className="phrase-input"
          style={{ flex: 1, margin: 0 }}
          type="number"
          min={1}
          max={255}
          step={1}
          value={props.systemId}
          onChange={(e) => props.onSystemId(Number(e.target.value))}
          aria-label="SITL MAVLink system ID"
          disabled={props.connected}
        />
      </label>
      <label className="motor-row">
        <span className="motor-label" style={{ width: 92 }}>Component ID</span>
        <input
          className="phrase-input"
          style={{ flex: 1, margin: 0 }}
          type="number"
          min={1}
          max={255}
          step={1}
          value={props.componentId}
          onChange={(e) => props.onComponentId(Number(e.target.value))}
          aria-label="SITL MAVLink component ID"
          disabled={props.connected}
        />
      </label>
      <label className="motor-row">
        <span className="motor-label" style={{ width: 92 }}>Output bank</span>
        <select
          className="phrase-input"
          style={{ flex: 1, margin: 0 }}
          value={props.port}
          onChange={(e) => props.onPort(e.target.value as ServoPort)}
          aria-label="SITL output bank"
          disabled={props.connected}
        >
          <option value="main">MAIN</option>
          <option value="aux">AUX</option>
        </select>
      </label>
      {props.error && (
        <div className="estop-note" role="alert" style={{ color: 'var(--mg-error)' }}>
          {props.error}
        </div>
      )}
      {props.connected ? (
        <button
          className="mode-btn"
          style={{ width: '100%', marginTop: 6 }}
          onClick={props.onDisconnect}
        >
          Disconnect SITL
        </button>
      ) : (
        <button
          className="mode-btn"
          style={{ width: '100%', marginTop: 6 }}
          onClick={props.onConnect}
          disabled={!validIds || props.endpoint.trim() === ''}
        >
          Connect SITL
        </button>
      )}
    </>
  )
}

const IDLE: ActuatorStatus = {
  mode_label: '',
  connected: false,
  fc_alive: false,
  armed: false,
  custom_mode: 0,
  autopilot_px4: false,
  endpoint: null,
  tick_hz: 10,
  session: 'idle',
  sitl_connected: false,
  sitl_endpoint: null,
  sitl_source: null,
  sitl_feed_age_s: null,
  target_connected: false,
  target_fc_alive: false,
  target_endpoint: null,
  target_system_id: null,
  com_mot_test_en: null,
  sdlog_mode: null,
  sdlog_backend: null,
  input_values: [],
  input_pwm: [],
  output_values: [],
  fc_output_pwm: [],
  output_map: { motors: [], state: 'unknown', issues: [] },
  pwm_models: [],
  sd_capture: {
    phase: 'idle',
    test_id: null,
    remote_path: null,
    logger: null,
    sdlog_mode: null,
    sdlog_backend: null,
    last_error: null,
  },
  run: { status: null, manifest_path: null },
}

export function ActuatorApp() {
  const tauri = isTauri()
  // Collect scope samples for the window's whole lifetime, so the history
  // is there when the operator looks at the scope below the controls.
  useScopeFeed()
  const [status, setStatus] = useState<ActuatorStatus>(IDLE)
  const [load, setLoad] = useState<Load>('none')
  const [source, setSource] = useState<Source>('manual')
  /** Unlocked = the operator removed the safety cover. Any load change
   * re-locks instantly (§8.2: the confirmation binds to session+load). */
  const [unlocked, setUnlocked] = useState(false)
  /** Latched emergency: stays until an explicit reset, never auto-clears. */
  const [emergency, setEmergency] = useState(false)
  /** Refusal from the backend on the last start attempt (e.g. no serial
   * target connected) — shown in the header until cleared. */
  const [startError, setStartError] = useState<string | null>(null)
  /** `COM_MOT_TEST_EN` from the parameter cache: true/false, null = unknown
   * (never pulled). false disables motor tests and offers the jump. */
  const [motTestEn, setMotTestEn] = useState<boolean | null>(null)
  // Manual source state.
  const [values, setValues] = useState<number[]>(() => new Array(MOTOR_COUNT).fill(0))
  const [sync, setSync] = useState(true)
  // Preset source state.
  const [preset, setPreset] = useState<PresetKind>('ramp')
  const [amplitude, setAmplitude] = useState(0.5)
  const [frequency, setFrequency] = useState(0.5)
  const [duration, setDuration] = useState(5)
  // Realtime forward (revised D3): the SITL node and output bank selected
  // from the dedicated source connection.
  const [sitlEndpoint, setSitlEndpoint] = useState('udpin:0.0.0.0:14540')
  const [sitlSystemId, setSitlSystemId] = useState(1)
  const [sitlComponentId, setSitlComponentId] = useState(1)
  const [sitlPort, setSitlPort] = useState<ServoPort>('main')
  const [sitlConnectError, setSitlConnectError] = useState<string | null>(null)
  const [mappingError, setMappingError] = useState<string | null>(null)
  // The motor test's own target link: the real PX4 over serial (the main
  // GCS keeps its own link, e.g. UDP SITL).
  const [targetEndpoint, setTargetEndpoint] = useState('serial:/dev/ttyACM0:115200')
  const [targetSystemId, setTargetSystemId] = useState(1)
  const [targetConnectError, setTargetConnectError] = useState<string | null>(null)
  // Why the last session stopped (armed / heartbeat_stale / acks_denied / …).
  const [stopReason, setStopReason] = useState<string | null>(null)
  // Confirmation state.
  const [phrase, setPhrase] = useState('')
  // Mag alarm (A4b): enable + threshold are operator inputs; the debounce/
  // hysteresis/cooldown state machine runs in core. `magAlarm` mirrors the
  // core state via the actuator_telemetry/alarm events.
  const [alarmEnabled, setAlarmEnabled] = useState(false)
  const [alarmThreshold, setAlarmThreshold] = useState(55_000)
  const [magAlarm, setMagAlarm] = useState(false)
  const [magTotal, setMagTotal] = useState<number | null>(null)
  // Shared mag calibration (same file as the Signal Inspector): when loaded,
  // the alarm's total field and the inspector's mag traces are A·(m−b).
  const [calLoaded, setCalLoaded] = useState(false)
  // On-board SD-card log: the capture panel reads SDLOG_MODE/BACKEND from the
  // target PX4 but never writes them.
  const [captureBusy, setCaptureBusy] = useState(false)
  const [captureError, setCaptureError] = useState<string | null>(null)
  // SD-log list + download (feature 2): `actuator_logs` events.
  const [logEntries, setLogEntries] = useState<LogEntryInfo[]>([])
  const [logsListed, setLogsListed] = useState(false)
  const [logDownloads, setLogDownloads] = useState<Map<number, { fetched: number; total: number; path?: string }>>(new Map())
  const [logsBusy, setLogsBusy] = useState(false)
  const [logsError, setLogsError] = useState<string | null>(null)
  // Responsive layout (Task 9): collapsible scope legend and expandable
  // logging details leave the plots as the flexible region at narrow widths.
  const [legendOpen, setLegendOpen] = useState(true)
  const [loggingExpanded, setLoggingExpanded] = useState(false)
  const audioCtxRef = useRef<AudioContext | null>(null)
  const beepTimer = useRef<number | undefined>(undefined)

  const valuesRef = useRef(values)
  valuesRef.current = values
  const lastSend = useRef(0)

  const running = status.session === 'running' || status.session === 'stopping'

  // Emergency stop: one gesture, always available. `Esc` is captured on the
  // window in the capture phase, so it works while focus is inside a slider
  // or text input; Space is deliberately NOT bound (plan 2.2-4).
  const estop = useCallback(() => {
    setEmergency(true)
    if (tauri) void invoke('actuator_estop').catch(() => {})
  }, [tauri])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        estop()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [estop])

  // Live status from the shell; session pushes arrive faster via events.
  // The scope's FC/mag/ESC curves ride the serial target link only; when the
  // target drops, drop the buffered samples so a stale non-zero trace never
  // looks live (the manual Clear button drops the same store).
  const prevTargetConnectedRef = useRef(false)
  useEffect(() => {
    if (!tauri) return
    const poll = async () => {
      try {
        const s = await invoke<ActuatorStatus>('actuator_status')
        setStatus(s)
        if (s.target_connected && s.target_system_id !== null) {
          setTargetSystemId(s.target_system_id)
        }
        if (s.session === 'emergency') setEmergency(true)
        if (prevTargetConnectedRef.current && !s.target_connected) {
          scopeStore.clear()
        }
        prevTargetConnectedRef.current = s.target_connected
      } catch {
        /* window opening before the link is up: keep the last status */
      }
    }
    void poll()
    const id = setInterval(poll, STATUS_POLL_MS)
    return () => clearInterval(id)
  }, [tauri])

  useEffect(() => {
    if (!tauri) return
    let disposed = false
    let un: (() => void) | undefined
    void (async () => {
      try {
        un = await listen<{ kind: 'state' | 'stopped'; reason?: string }>(
          'actuator_event',
          (e) => {
            if (disposed) return
            // A stopped session carries the reason — surface it so a run that
            // "goes straight back to idle" is diagnosable (armed, heartbeat
            // stale, acks denied, ...).
            if (e.payload.kind === 'stopped' && e.payload.reason) {
              setStopReason(e.payload.reason)
            }
            void invoke<ActuatorStatus>('actuator_status')
              .then(setStatus)
              .catch(() => {})
          },
        )
      } catch {
        /* events unavailable: the poll still runs */
      }
    })()
    return () => {
      disposed = true
      un?.()
    }
  }, [tauri])

  // PM9 (parameters plan): read COM_MOT_TEST_EN from the parameter cache —
  // the value lands there after a Pull all or a lazy read in the Parameters
  // window. null = not in the cache yet (never pulled), false = disabled.
  useEffect(() => {
    if (!tauri) return
    const poll = async () => {
      try {
        const entry = await invoke<ParamEntry | null>('params_cache_get', {
          name: MOT_TEST_EN,
        })
        setMotTestEn(entry ? Number('int' in entry.value ? entry.value.int : entry.value.real) !== 0 : null)
      } catch {
        /* not connected: keep the last known state */
      }
    }
    void poll()
    const id = setInterval(poll, MOT_TEST_EN_POLL_MS)
    return () => clearInterval(id)
  }, [tauri])

  /** Alarm beep: a synthesized square wave through WebAudio — works on
   * Ubuntu and Windows alike, no bundled asset. */
  const beep = useCallback(() => {
    try {
      audioCtxRef.current ??= new AudioContext()
      const ctx = audioCtxRef.current
      const osc = ctx.createOscillator()
      const gain = ctx.createGain()
      osc.type = 'square'
      osc.frequency.value = 880
      gain.gain.value = 0.08
      osc.connect(gain).connect(ctx.destination)
      osc.start()
      osc.stop(ctx.currentTime + 0.18)
    } catch {
      /* no audio device: the visual alarm still runs */
    }
  }, [])

  /** Alarm audio loop: beep at ~2 Hz while latched. */
  useEffect(() => {
    if (!magAlarm || !alarmEnabled) return
    beep()
    beepTimer.current = window.setInterval(beep, 500)
    return () => {
      if (beepTimer.current !== undefined) {
        window.clearInterval(beepTimer.current)
        beepTimer.current = undefined
      }
    }
  }, [magAlarm, alarmEnabled, beep])

  const acknowledgeAlarm = () => {
    setMagAlarm(false)
    if (tauri) void invoke('actuator_acknowledge_alarm').catch(() => {})
  }

  /** Load a mag calibration JSON (shared with the Signal Inspector): the
   * alarm's total field and the inspector's mag traces become A·(m−b). */
  const loadMagCal = async () => {
    try {
      const f = await openDialog({
        multiple: false,
        filters: [{ name: 'Mag calibration', extensions: ['json'] }],
      })
      if (!f || typeof f !== 'string') return
      const text = await readTextFile(f)
      await invoke('set_mag_calibration', { json: text })
      setCalLoaded(true)
    } catch {
      setCalLoaded(false)
    }
  }

  const clearMagCal = async () => {
    try {
      await invoke('set_mag_calibration', { json: null })
      setCalLoaded(false)
    } catch {
      /* keep the local flag */
    }
  }

  // Mag telemetry + alarm events; the threshold change is pushed to core.
  useEffect(() => {
    if (!tauri) return
    let disposed = false
    let unTelemetry: (() => void) | undefined
    let unAlarm: (() => void) | undefined
    void (async () => {
      try {
        unTelemetry = await listen<{
          mag_total_nt: number | null
          alarm: string
        }>('actuator_telemetry', (e) => {
          if (disposed) return
          setMagTotal(e.payload.mag_total_nt)
          setMagAlarm(e.payload.alarm === 'alarm')
        })
        unAlarm = await listen('actuator_alarm', () => {
          if (disposed) return
          setMagAlarm(true)
        })
      } catch {
        /* events unavailable */
      }
    })()
    return () => {
      disposed = true
      unTelemetry?.()
      unAlarm?.()
    }
  }, [tauri])

  // Push the threshold (and enable) to the core watchdog when they change.
  useEffect(() => {
    if (!tauri) return
    void invoke('actuator_set_alarm_config', {
      config: {
        threshold_nt: alarmEnabled ? alarmThreshold : Number.MAX_SAFE_INTEGER,
        debounce_s: 1.0,
        hysteresis_nt: 50.0,
        cooldown_s: 30.0,
        ema_alpha: 0.2,
      },
    }).catch(() => {})
  }, [tauri, alarmEnabled, alarmThreshold])

  // Feature 2: SD-log list/download over the serial target — subscribe to the
  // `actuator_logs` events and seed from `actuator_logs_status`.
  useEffect(() => {
    if (!tauri) return
    let disposed = false
    let un: (() => void) | undefined
    void (async () => {
      try {
        un = await listen<{
          kind: 'list' | 'progress' | 'downloaded' | 'failed'
          entries?: Array<{ id: number; time_utc: number; size: number }>
          id?: number
          fetched?: number
          total?: number
          path?: string
          message?: string
        }>('actuator_logs', (e) => {
          if (disposed) return
          const p = e.payload
          if (p.kind === 'list') {
            setLogEntries(p.entries ?? [])
            setLogsListed(true)
            setLogDownloads(new Map())
            setLogsError(null)
          } else if (p.kind === 'progress') {
            const id = p.id ?? 0
            setLogDownloads((prev) => {
              const next = new Map(prev)
              next.set(id, { fetched: p.fetched ?? 0, total: p.total ?? 0 })
              return next
            })
          } else if (p.kind === 'downloaded') {
            const id = p.id ?? 0
            setLogDownloads((prev) => {
              const next = new Map(prev)
              const cur = next.get(id)
              next.set(id, { fetched: cur?.fetched ?? 0, total: cur?.total ?? 0, path: p.path })
              return next
            })
            setLogsBusy(false)
          } else if (p.kind === 'failed') {
            setLogsBusy(false)
            setLogDownloads(new Map())
            setLogsError(p.message ?? 'failed')
          }
        })
      } catch {
        /* events unavailable */
      }
    })()
    void invoke<{
      entries: Array<{ id: number; time_utc: number; size: number }>
      error: string | null
    }>('actuator_logs_status')
      .then((s) => {
        if (disposed) return
        setLogEntries(s.entries ?? [])
        setLogsListed((s.entries ?? []).length > 0)
        setLogsError(s.error)
      })
      .catch(() => {})
    return () => {
      disposed = true
      un?.()
    }
  }, [tauri])

  // Capture events arrive while a shell exchange progresses, so failures and
  // existing recordings show up without waiting for the next status poll.
  useEffect(() => {
    if (!tauri) return
    let disposed = false
    let un: (() => void) | undefined
    void (async () => {
      try {
        un = await listen<{ kind: 'status'; status: SdCaptureStatus }>(
          'actuator_capture',
          (e) => {
            if (disposed || e.payload.kind !== 'status') return
            setStatus((s) => ({ ...s, sd_capture: e.payload.status }))
          },
        )
      } catch {
        /* events unavailable: the status poll still updates the panel */
      }
    })()
    return () => {
      disposed = true
      un?.()
    }
  }, [tauri])

  // Run boundaries (pre-roll done, POST-ROLL done, terminal) arrive as they
  // happen; the poll remains the fallback.
  useEffect(() => {
    if (!tauri) return
    let disposed = false
    let un: (() => void) | undefined
    void (async () => {
      try {
        un = await listen<RunStatus>('actuator_run', (e) => {
          if (disposed) return
          setStatus((s) => ({ ...s, run: { ...s.run, status: e.payload } }))
        })
      } catch {
        /* events unavailable: the status poll still updates the gate */
      }
    })()
    return () => {
      disposed = true
      un?.()
    }
  }, [tauri])

  const captureCommand = async (
    command: 'check' | 'start' | 'stop',
    testId?: string,
  ) => {
    if (!tauri) return
    setCaptureBusy(true)
    setCaptureError(null)
    try {
      const status = await invoke<SdCaptureStatus>(
        command === 'check'
          ? 'actuator_capture_verify'
          : command === 'start'
            ? 'actuator_capture_start'
            : 'actuator_capture_stop',
        command === 'start' ? { testId } : undefined,
      )
      setStatus((s) => ({ ...s, sd_capture: status }))
    } catch (e) {
      setCaptureError(String(e))
    } finally {
      setCaptureBusy(false)
    }
  }

  const startCapture = () =>
    captureCommand(
      'start',
      `test-${new Date().toISOString().replace(/[-:]/g, '').replace(/\..+$/, '')}`,
    )

  const requestLogs = async () => {
    if (!tauri) return
    setLogsBusy(true)
    setLogsError(null)
    setLogEntries([])
    setLogDownloads(new Map())
    setLogsListed(false)
    try {
      const entries = await invoke<LogEntryInfo[]>('actuator_logs_request_list')
      setLogEntries(entries)
      setLogsListed(true)
    } catch (e) {
      setLogsError(String(e))
    } finally {
      setLogsBusy(false)
    }
  }

  const downloadLog = async (entry: LogEntryInfo) => {
    if (!tauri) return
    setLogsBusy(true)
    setLogsError(null)
    try {
      const started = await invoke<boolean>('actuator_logs_download', {
        id: entry.id, size: entry.size, timeUtc: entry.time_utc,
      })
      if (!started) setLogsBusy(false)
    } catch (e) {
      setLogsBusy(false)
      setLogsError(String(e))
    }
  }

  const deleteLog = async (entry: LogEntryInfo) => {
    if (!tauri) return
    const { id, size, time_utc: timeUtc } = entry
    const mb = (size / 1024 / 1024).toFixed(1)
    if (
      !window.confirm(
        `Delete SD log #${id} (${mb} MB) on the target PX4? This cannot be undone.`,
      )
    ) {
      return
    }
    if (!window.confirm(`Last confirmation: permanently delete log #${id}?`)) return
    setLogsBusy(true)
    setLogsError(null)
    try {
      const entries = await invoke<LogEntryInfo[]>('actuator_logs_delete', { id, size, timeUtc })
      setLogEntries(entries)
      setLogDownloads(new Map())
      setLogsListed(true)
    } catch (e) {
      setLogsError(String(e))
    } finally {
      setLogsBusy(false)
    }
  }

  const eraseAllLogs = async () => {
    if (!tauri) return
    if (
      !window.confirm(
        'Delete ALL SD logs on the target PX4? Every flight log will be erased. This cannot be undone.',
      )
    ) {
      return
    }
    if (!window.confirm('Last confirmation: permanently delete every SD log?')) return
    setLogsBusy(true)
    setLogsError(null)
    try {
      const entries = await invoke<LogEntryInfo[]>('actuator_logs_erase_all')
      setLogEntries(entries)
      setLogDownloads(new Map())
      setLogsListed(true)
    } catch (e) {
      setLogEntries([])
      setLogDownloads(new Map())
      setLogsListed(false)
      setLogsError(String(e))
    } finally {
      setLogsBusy(false)
    }
  }

  const cancelLogs = async () => {
    if (!tauri) return
    try {
      await invoke('actuator_logs_cancel')
    } catch {
      /* not connected */
    }
    setLogsBusy(false)
    setLogDownloads(new Map())
  }

  const pushValues = useCallback(
    (next: number[]) => {
      if (!tauri || !running) return
      const now = performance.now()
      if (now - lastSend.current < SET_VALUES_MIN_INTERVAL_MS) return
      lastSend.current = now
      void invoke('actuator_set_values', { values: next }).catch(() => {})
    },
    [tauri, running],
  )

  const setMotor = (index: number, v: number) => {
    const next = sync ? values.map(() => v) : values.map((x, i) => (i === index ? v : x))
    setValues(next)
    pushValues(next)
  }

  const changeLoad = (next: Load) => {
    setLoad(next)
    setUnlocked(false)
    setPhrase('')
  }

  const resetEmergency = () => {
    setEmergency(false)
    setUnlocked(false)
    if (tauri) void invoke('actuator_reset_emergency').catch(() => {})
  }

  const stop = async () => {
    if (!tauri) return
    try {
      await invoke('actuator_stop')
    } catch (error) {
      setStartError(`Stop failed: ${String(error)}`)
    }
  }

  /** What still gates Start: unlock → (props: typed phrase) → hold. */
  const gate = useMemo(() => {
    if (emergency) return 'emergency' as const
    if (!unlocked) return 'unlock' as const
    if (load === 'props' && phrase.trim().toUpperCase() !== PROPS_PHRASE)
      return 'phrase' as const
    return 'ready' as const
  }, [emergency, unlocked, load, phrase])

  /** The one start action — dispatches by the selected source. */
  const start = async () => {
    if (!tauri || gate !== 'ready' || running) return
    setStartError(null)
    // The backend refuses (e.g. "no link — connect the serial PX4 first");
    // surface it instead of silently doing nothing.
    setStopReason(null)
    try {
      if (source === 'manual') {
        await invoke('actuator_start_manual', { load, values: valuesRef.current })
      } else if (source === 'preset') {
        await invoke('actuator_start_preset', {
        load,
        preset: {
          kind: preset,
          amplitude,
          frequency_hz: frequency,
          duration_s: duration,
        },
        })
      } else if (source === 'forward') {
        await invoke('actuator_start_forward', { load })
      }
    } catch (error) {
      setStartError(typeof error === 'string' ? error : String(error))
    }
  }

  const connectSitl = () => {
    if (!tauri) return
    setSitlConnectError(null)
    void invoke('actuator_connect_sitl', {
      source: {
        endpoint: sitlEndpoint,
        system_id: sitlSystemId,
        component_id: sitlComponentId,
        port: sitlPort,
      },
    }).catch((error: unknown) => {
      setSitlConnectError(String(error))
    })
  }
  const disconnectSitl = () => {
    if (!tauri) return
    setSitlConnectError(null)
    void invoke('actuator_disconnect_sitl').catch((error: unknown) => {
      setSitlConnectError(String(error))
    })
  }
  const refreshMapping = () => {
    if (!tauri) return
    setMappingError(null)
    void invoke('actuator_refresh_mapping').catch((error: unknown) => {
      setMappingError(String(error))
    })
  }

  // The capture panel reflects observed hardware state; it never writes the
  // restart-required SD parameters. A check refreshes both values.
  const sdCapture = status.sd_capture
  const captureActive =
    sdCapture.phase === 'recording' || sdCapture.phase === 'existing_recording'
  const captureSettling = ['verifying', 'starting', 'stopping', 'finalizing'].includes(
    sdCapture.phase,
  )
  const captureCanStop =
    sdCapture.phase === 'recording' ||
    sdCapture.phase === 'existing_recording' ||
    sdCapture.phase === 'failed' ||
    sdCapture.phase === 'complete'
  const hardwareMode = sdCapture.sdlog_mode ?? status.sdlog_mode
  const hardwareBackend = sdCapture.sdlog_backend ?? status.sdlog_backend
  const run = status.run.status
  const runBlocksStart =
    run != null &&
    ['starting_capture', 'preroll', 'motor_running', 'postroll', 'stopping_capture'].includes(
      run.phase,
    )
  const runLabel = run ? run.phase.replaceAll('_', ' ') : 'no run'
  const runHint =
    run?.phase === 'preroll'
      ? 'Recording — waiting for the 1 s pre-roll before START.'
      : run?.phase === 'starting_capture'
        ? 'Confirming the new SD file.'
        : run?.phase === 'postroll'
          ? 'Motors released — post-roll before logger off.'
          : run?.phase === 'stopping_capture'
            ? 'Confirming logger off.'
            : run?.error
              ? String(run.error.kind).replaceAll('_', ' ')
              : null

  return (
    <div className={`actuator-grid ${magAlarm && alarmEnabled ? 'mag-alarm-flash' : ''}`}>
      <header className="actuator-header">
        <strong style={{ fontSize: 13 }}>Motor / Mag Interference Test</strong>
        <span className="row" style={{ gap: 6 }}>
          <span className="section-title">FC</span>
          <span className={`mono ${status.fc_alive ? 'text-ok' : 'text-error'}`}>
            {status.fc_alive ? 'ONLINE' : 'NO LINK'}
          </span>
        </span>
        <span className="row" style={{ gap: 6 }}>
          <span className="section-title">ARMED</span>
          <span className={`mono ${status.armed ? 'text-error' : 'text-ok'}`}>
            {status.armed ? 'YES' : 'NO'}
          </span>
        </span>
        <span className="row" style={{ gap: 6 }}>
          <span className="section-title">RATE</span>
          <span className="mono">{status.tick_hz.toFixed(0)} Hz</span>
        </span>
        {emergency ? (
          <span className="unlock-banner" style={{ color: 'var(--mg-error)' }}>
            EMERGENCY STOP — RESET REQUIRED
          </span>
        ) : startError ? (
          <span className="unlock-banner" style={{ color: 'var(--mg-error)' }}>
            {startError}
          </span>
        ) : unlocked ? (
          <span className="unlock-banner">TEST UNLOCKED</span>
        ) : (
          <span />
        )}
        {emergency && (
          <button className="estop-btn" onClick={resetEmergency} style={{ marginLeft: 0 }}>
            RESET
          </button>
        )}
        <button
          className={`estop-btn ${emergency ? 'latched' : ''}`}
          onClick={estop}
          title="Stop all motors immediately (Esc)"
        >
          EMERGENCY STOP
        </button>
      </header>

      {/* Left: the SITL input source (SERVO_OUTPUT_RAW → the forward slot). */}
      <aside className="actuator-left">
        <div className="section-title">Input — SITL</div>
        <div className="card">
          <h3>SITL source (SERVO_OUTPUT_RAW)</h3>
          <SitlSourceFields
            endpoint={sitlEndpoint}
            onEndpoint={setSitlEndpoint}
            systemId={sitlSystemId}
            onSystemId={setSitlSystemId}
            componentId={sitlComponentId}
            onComponentId={setSitlComponentId}
            port={sitlPort}
            onPort={setSitlPort}
            connected={status.sitl_connected}
            error={sitlConnectError}
            onConnect={connectSitl}
            onDisconnect={disconnectSitl}
          />
          <div className="row" style={{ marginTop: 6 }}>
            <span className="text-muted">Source link</span>
            <span className={`mono ${status.sitl_connected ? 'text-ok' : 'text-error'}`}>
              {status.sitl_connected ? 'connected' : 'down'}
            </span>
          </div>
          <div className="row">
            <span className="text-muted">Feed age</span>
            <span className="mono">
              {status.sitl_feed_age_s != null
                ? `${status.sitl_feed_age_s.toFixed(1)} s`
                : 'no data yet'}
            </span>
          </div>
          <div className="motor-bars" style={{ marginTop: 6 }}>
            {Array.from({ length: MOTOR_COUNT }, (_, i) => {
              const v = status.input_values[i] ?? 0
              const us = status.input_pwm[i]
              return (
                <div key={i} className="motor-bar">
                  <span className="mono motor-label">M{i + 1}</span>
                  <div className="bar-track">
                    <div className="bar-fill input" style={{ width: `${v * 100}%` }} />
                  </div>
                  <span className="mono motor-value">{(v * 100).toFixed(0)}%</span>
                  <span className="mono motor-pwm" title="SITL SERVO_OUTPUT_RAW">
                    {us != null ? `${us} µs` : '—'}
                  </span>
                </div>
              )
            })}
          </div>
          <div className="estop-note" style={{ marginTop: 4 }}>
            Selected SITL SERVO_OUTPUT_RAW port 1–4; % = (µs − 1000) / 1000
            (1000 µs = 0%, 2000 µs = 100%) → the forward slot.
          </div>
        </div>
      </aside>

      {/* Middle: the command source and safety controls. */}
      <main className="actuator-mid">
        <div className="state-strip">
          <span
            className="mono state-word"
            style={{
              color: emergency
                ? 'var(--mg-error)'
                : running
                  ? 'var(--mg-ok)'
                  : 'var(--mg-muted)',
            }}
          >
            {emergency ? 'EMERGENCY' : running ? 'RUNNING' : unlocked ? 'READY' : 'LOCKED'}
          </span>
          <span className="estop-note">
            {running
              ? `${SOURCES.find((s) => s.id === source)?.label} · ${status.tick_hz.toFixed(0)} Hz · window on top`
              : stopReason
                ? `Stopped: ${stopReason} · Esc = emergency stop`
                : 'Motor commands stopped · Esc = emergency stop'}
          </span>
          <span className="view-tabs">
            {running && (
              <button className="mode-btn view-tab stop-tab" onClick={stop} title="Stop sending">
                STOP
              </button>
            )}
          </span>
        </div>
        <div className="controls-grid">
          <div className="card">
            <h3>Prop load</h3>
            <div className="btn-row">
              {LOADS.map((l) => (
                <button
                  key={l.id}
                  className={`mode-btn ${load === l.id ? 'active' : ''}`}
                  onClick={() => changeLoad(l.id)}
                  title="Declared by the operator; there is no sensor to detect it"
                >
                  {l.label}
                </button>
              ))}
            </div>
            <div className="estop-note" style={{ marginTop: 6 }}>
              {load === 'props'
                ? 'Full props: type START PROPS, then hold Start for 3 s. Vehicle must be fixed to a rig.'
                : load === 'plate'
                  ? 'Flat plates: same start path as no props.'
                  : 'No props: safest configuration.'}
            </div>
          </div>

          <div className="card">
            <h3>Command source</h3>
            <div className="btn-row">
              {SOURCES.map((s) => (
                <button
                  key={s.id}
                  className={`mode-btn ${source === s.id ? 'active' : ''}`}
                  disabled={running}
                  onClick={() => setSource(s.id)}
                  title={running ? 'Stop the test before switching source' : s.hint}
                >
                  {s.label}
                </button>
              ))}
            </div>
            <div className="estop-note" style={{ marginTop: 6 }}>
              {SOURCES.find((s) => s.id === source)?.hint}
            </div>
          </div>

        {/* Source panel: only the selected source's controls. */}
        {source === 'manual' && (
          <div className="card">
            <h3>Motors</h3>
            <label className="row" style={{ marginBottom: 6 }}>
              <span>Synchronize all</span>
              <input
                type="checkbox"
                checked={sync}
                disabled={emergency}
                onChange={(e) => setSync(e.target.checked)}
              />
            </label>
            {values.map((v, i) => (
              <label key={i} className="motor-row">
                <span className="mono motor-label">M{i + 1}</span>
                <input
                  type="range"
                  min={0}
                  max={1}
                  step={0.01}
                  value={v}
                  disabled={emergency}
                  onChange={(e) => setMotor(i, Number(e.target.value))}
                />
                <span className="mono motor-value">{(v * 100).toFixed(0)}%</span>
              </label>
            ))}
          </div>
        )}

        {source === 'preset' && (
          <div className="card">
            <h3>Waveform</h3>
            <div className="btn-row">
              {PRESETS.map((p) => (
                <button
                  key={p.id}
                  className={`mode-btn ${preset === p.id ? 'active' : ''}`}
                  onClick={() => setPreset(p.id)}
                >
                  {p.label}
                </button>
              ))}
            </div>
            <label className="motor-row" style={{ marginTop: 8 }}>
              <span className="motor-label" style={{ width: 52 }}>Ampl.</span>
              <input
                type="range"
                min={0}
                max={1}
                step={0.01}
                value={amplitude}
                disabled={emergency}
                onChange={(e) => setAmplitude(Number(e.target.value))}
              />
              <span className="mono motor-value">{(amplitude * 100).toFixed(0)}%</span>
            </label>
            <label className="motor-row">
              <span className="motor-label" style={{ width: 52 }}>Freq.</span>
              <input
                className="phrase-input"
                style={{ flex: 1, margin: 0 }}
                type="number"
                min={0.01}
                max={20}
                step={0.1}
                value={frequency}
                disabled={emergency}
                onChange={(e) => setFrequency(Number(e.target.value))}
              />
              <span className="mono motor-value">Hz</span>
            </label>
            <label className="motor-row">
              <span className="motor-label" style={{ width: 52 }}>Dur.</span>
              <input
                className="phrase-input"
                style={{ flex: 1, margin: 0 }}
                type="number"
                min={0.5}
                max={300}
                step={0.5}
                value={duration}
                disabled={emergency}
                onChange={(e) => setDuration(Number(e.target.value))}
              />
              <span className="mono motor-value">s</span>
            </label>
            <div className="estop-note" style={{ marginTop: 4 }}>
              All motors in sync; runs its duration, then stops itself.
            </div>
          </div>
        )}

        {source === 'forward' && (
          <div className="card">
            <h3>Realtime forward</h3>
            <SitlSourceFields
              endpoint={sitlEndpoint}
              onEndpoint={setSitlEndpoint}
              systemId={sitlSystemId}
              onSystemId={setSitlSystemId}
              componentId={sitlComponentId}
              onComponentId={setSitlComponentId}
              port={sitlPort}
              onPort={setSitlPort}
              connected={status.sitl_connected}
              error={sitlConnectError}
              onConnect={connectSitl}
              onDisconnect={disconnectSitl}
            />
            <div className="row" style={{ marginTop: 6 }}>
              <span className="text-muted">Source link</span>
              <span className={`mono ${status.sitl_connected ? 'text-ok' : 'text-error'}`}>
                {status.sitl_connected ? 'connected' : 'down'}
              </span>
            </div>
            <div className="row">
              <span className="text-muted">Feed age</span>
              <span className="mono">
                {status.sitl_feed_age_s != null
                  ? `${status.sitl_feed_age_s.toFixed(1)} s`
                  : 'no data yet'}
              </span>
            </div>
            <div className="estop-note" style={{ marginTop: 4 }}>
              SITL outputs 1–4 from the selected node and bank → motors 1–4.
              Values pass the same interlocks (slew, cap, disarmed-only) as
              manual control.
            </div>
          </div>
        )}

        <div className="card">
          <h3>Safety</h3>
          <button
            className="mode-btn"
            onClick={() => (emergency ? undefined : setUnlocked((u) => !u))}
            disabled={emergency || running}
            style={{ width: '100%' }}
          >
            {unlocked ? 'Re-lock test' : 'Unlock test'}
          </button>
          {load === 'props' && unlocked && !running && (
            <input
              className="phrase-input"
              placeholder={PROPS_PHRASE}
              value={phrase}
              onChange={(e) => setPhrase(e.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
          )}
            <StartStopButton
              running={running}
              holdMs={load === 'props' ? PROPS_HOLD_MS : 0}
              onStart={start}
              onStop={stop}
              disabled={
                gate !== 'ready' ||
                (source === 'forward' && !status.sitl_connected) ||
                !status.target_connected ||
                !status.target_fc_alive ||
                status.armed ||
                runBlocksStart
              }
              title={
                gate === 'emergency'
                  ? 'Reset the emergency latch first'
                  : gate === 'unlock'
                    ? 'Unlock the test first'
                    : gate === 'phrase'
                      ? `Type ${PROPS_PHRASE} first`
                      : !status.target_connected
                        ? 'Connect the serial PX4 first'
                        : !status.target_fc_alive
                          ? 'Waiting for the target PX4 heartbeat'
                          : status.armed
                            ? 'Disarm the target PX4 first'
                            : runBlocksStart
                              ? runHint ?? 'Wait for the active test run'
                              : source === 'forward'
                                ? 'Connect the SITL source first'
                                : load === 'props'
                                  ? 'Hold for 3 seconds'
                                  : 'Start sending'
              }
            />
          <div className="estop-note" style={{ marginTop: 6 }}>
            {running
              ? 'Sending — Esc or the red button stops everything.'
              : 'No arming required: the test runs while the FC is disarmed.'}
          </div>
        </div>
      </div>
      <SignalScope
        sitlConnected={status.sitl_connected}
        targetConnected={status.target_connected}
        calLoaded={calLoaded}
        legendOpen={legendOpen}
        onToggleLegend={() => setLegendOpen((v) => !v)}
      />
      </main>

      {/* Right: the actuator output to the real PX4 (serial). */}
      <aside className="actuator-right">
        <div className="section-title">Output — PX4 (serial)</div>
        <div className="card">
          <h3>Actuator output (ACTUATOR_TEST)</h3>
          <input
            className="phrase-input"
            style={{ width: '100%', margin: 0 }}
            value={targetEndpoint}
            onChange={(e) => setTargetEndpoint(e.target.value)}
            placeholder="serial:/dev/ttyACM0:115200"
            aria-label="Target PX4 serial endpoint"
            spellCheck={false}
          />
          <label className="motor-row" style={{ marginTop: 6 }}>
            <span className="motor-label" style={{ width: 92 }}>System ID</span>
            <input
              className="phrase-input"
              style={{ flex: 1, margin: 0 }}
              type="number"
              min={1}
              max={255}
              step={1}
              value={targetSystemId}
              onChange={(e) => {
                setTargetSystemId(Number(e.target.value))
                setTargetConnectError(null)
              }}
              aria-label="Target MAVLink system ID"
              disabled={status.target_connected}
            />
          </label>
          <div className="estop-note" style={{ marginTop: 4 }}>
            Routing only — this does not change the vehicle's configured ID.
          </div>
          {targetConnectError && (
            <div className="estop-note" role="alert" style={{ color: 'var(--mg-error)' }}>
              {targetConnectError}
            </div>
          )}
          {status.target_connected ? (
            <button
              className="mode-btn"
              style={{ width: '100%', marginTop: 6 }}
              onClick={() => tauri && void invoke('actuator_disconnect_target').catch(() => {})}
            >
              Disconnect PX4
            </button>
          ) : (
            <button
              className="mode-btn"
              style={{ width: '100%', marginTop: 6 }}
              onClick={() => {
                setTargetConnectError(null)
                void invoke('actuator_connect_target', {
                  endpoint: targetEndpoint,
                  targetSystemId,
                }).catch((error: unknown) => {
                  setTargetConnectError(String(error))
                })
              }}
              disabled={!Number.isInteger(targetSystemId) || targetSystemId < 1 || targetSystemId > 255}
            >
              Connect PX4 (serial)
            </button>
          )}
          <div className="row" style={{ marginTop: 6 }}>
            <span className="text-muted">Target link</span>
            <span className={`mono ${status.target_connected ? 'text-ok' : 'text-error'}`}>
              {status.target_connected ? 'connected' : 'down'}
            </span>
          </div>
          <div className="row">
            <span className="text-muted">Target heartbeat</span>
            <span
              className={`mono ${
                !status.target_connected
                  ? ''
                  : status.target_fc_alive
                    ? 'text-ok'
                    : 'text-error'
              }`}
            >
              {!status.target_connected
                ? '—'
                : status.target_fc_alive
                  ? 'alive'
                  : 'lost'}
            </span>
          </div>
          <div className="row">
            <span className="text-muted">Primary GCS link</span>
            <span className="mono">{status.fc_alive ? 'heartbeat alive' : 'heartbeat lost'}</span>
          </div>
          <div className="row">
            <span className="text-muted">Target system ID</span>
            <span className="mono">
              {status.target_connected ? status.target_system_id : 'primary link default (1)'}
            </span>
          </div>
          <div className="row">
            <span className="text-muted">FC mode</span>
            <span className="mono">
              {status.autopilot_px4 && status.mode_label ? status.mode_label : '—'}
            </span>
          </div>
          <div className="row">
            <span className="text-muted">Armed</span>
            <span className="mono">{status.armed ? 'YES — sending forbidden' : 'no'}</span>
          </div>
          <div className="row">
            <span className="text-muted">COM_MOT_TEST_EN</span>
            <span className="mot-en-light">
              <span
                className={`light ${
                  !status.target_connected
                    ? 'dim'
                    : status.com_mot_test_en === 1
                      ? 'on'
                      : 'off'
                }`}
                aria-hidden
              />
              <span className="mono">
                {!status.target_connected
                  ? 'no target'
                  : status.com_mot_test_en === 1
                    ? 'enabled'
                    : status.com_mot_test_en == null
                      ? 'unknown'
                      : 'disabled'}
              </span>
            </span>
          </div>
          <div className="row" style={{ marginTop: 6 }}>
            <span className="text-muted">Output map</span>
            <span
              className={`mono ${
                status.output_map.state === 'complete'
                  ? 'text-ok'
                  : status.output_map.state === 'invalid'
                    ? 'text-error'
                    : ''
              }`}
            >
              {status.output_map.state}
            </span>
          </div>
          <div className="estop-note" style={{ marginTop: 2 }}>
            {status.output_map.motors.length === 0
              ? 'not read yet'
              : status.output_map.motors
                  .map((assignment) =>
                    assignment.channel
                      ? `M${assignment.motor} ${channelLabel(assignment.channel)}`
                      : `M${assignment.motor} unmapped`,
                  )
                  .join(' · ')}
          </div>
          {status.pwm_models.length > 0 && (
            <div className="estop-note" style={{ marginTop: 2 }}>
              {status.pwm_models
                .map(
                  (model, i) =>
                    `M${i + 1} ${model.min_us}–${model.max_us}${
                      model.reversed ? ' rev' : ''
                    } a=${model.thrust_factor.toFixed(2)}${
                      model.disabled ? ' dis' : ''
                    }`,
                )
                .join(' · ')}
            </div>
          )}
          {status.output_map.issues.length > 0 && (
            <div className="estop-note" style={{ marginTop: 2 }}>
              {status.output_map.issues.length} assignment issue
              {status.output_map.issues.length === 1 ? '' : 's'} — refresh after
              checking PWM_MAIN/AUX_FUNCx.
            </div>
          )}
          {mappingError && (
            <div className="estop-note" role="alert" style={{ color: 'var(--mg-error)' }}>
              {mappingError}
            </div>
          )}
          <button
            className="mode-btn"
            style={{ width: '100%', marginTop: 6 }}
            onClick={refreshMapping}
            disabled={!status.target_connected || running}
          >
            Refresh mapping
          </button>
          <div className="motor-bars" style={{ marginTop: 6 }}>
            {Array.from({ length: MOTOR_COUNT }, (_, i) => {
              const v = status.output_values[i] ?? 0
              const actualUs = status.fc_output_pwm[i]
              const assignment = assignmentFor(status.output_map, i + 1)
              const channel = assignment?.channel
              return (
                <div key={i} className="motor-bar">
                  <span
                    className="mono motor-label"
                    title={channel ? `Feedback from ${channelLabel(channel)}` : 'No mapped output'}
                  >
                    <span>{`M${i + 1}`}</span>
                    <span style={{ fontSize: 9, color: 'var(--mg-muted)' }}>
                      {channel ? channelLabel(channel) : 'unmapped'}
                    </span>
                  </span>
                  <div className="bar-track">
                    <div className="bar-fill output" style={{ width: `${v * 100}%` }} />
                  </div>
                  <span className="mono motor-value" title={`${(v * 100).toFixed(1)}%`}>
                    {(v * 100).toFixed(0)}%
                  </span>
                  <span className="mono motor-pwm" title="Mapped PX4 SERVO_OUTPUT_RAW">
                    {actualUs != null ? `${actualUs} µs` : '—'}
                  </span>
                </div>
              )
            })}
          </div>
          <div className="estop-note" style={{ marginTop: 4 }}>
            Every ACTUATOR_TEST goes out over this serial link to the real PX4.
            % = the sent command; µs = that motor's mapped physical output (not
            a linear mapping of the command).
          </div>
        </div>
        {/* On-board SD-card log: read the restart-required SD parameters and
         * control the target's file logger; list/download the FC's SD logs. */}
        <div className={`card logging-card${loggingExpanded ? ' logging-expanded' : ''}`}>
          <h3>On-board log</h3>
          <div className="logging-summary">
            <span className="estop-note">
              {sdCapture.phase === 'recording'
                ? 'Recording'
                : sdCapture.phase === 'existing_recording'
                  ? 'Existing recording'
                  : `${logEntries.length} SD log${logEntries.length === 1 ? '' : 's'}`}
            </span>
            <button
              className="mode-btn"
              onClick={() => setLoggingExpanded((v) => !v)}
              title={loggingExpanded ? 'Collapse logging details' : 'Expand logging details'}
            >
              {loggingExpanded ? 'Collapse ▲' : 'Expand ▼'}
            </button>
          </div>
          <div className="logging-body">
          <div className="row">
            <span className="text-muted">SDLOG_MODE</span>
            <span className="mono" title="Read from the target PX4; changes require a reboot">
              {!status.target_connected ? 'no target' : sdlogModeLabel(hardwareMode)}
            </span>
          </div>
          <div className="row">
            <span className="text-muted">SDLOG_BACKEND</span>
            <span className="mono" title="Read from the target PX4; changes require a reboot">
              {!status.target_connected ? 'no target' : sdlogBackendLabel(hardwareBackend)}
            </span>
          </div>
          <div className="estop-note" style={{ marginTop: 4 }}>
            These values are read-only here and may differ from the running logger
            until the PX4 reboots. Check logger refreshes them from the hardware.
          </div>

          <div className="row" style={{ marginTop: 8 }}>
            <span className="text-muted">SD capture</span>
            <span
              className={`mono ${
                sdCapture.phase === 'recording'
                  ? 'text-ok'
                  : sdCapture.phase === 'failed'
                    ? 'text-error'
                    : sdCapture.phase === 'existing_recording'
                      ? 'text-muted'
                      : ''
              }`}
            >
              {sdCapture.phase.replaceAll('_', ' ')}
            </span>
          </div>
          {sdCapture.phase === 'existing_recording' && (
            <div className="estop-note" style={{ marginTop: 4 }}>
              The target is already recording a file this test did not start.
              Capture is independent of START; stop logging manually when finished.
            </div>
          )}
          {(sdCapture.test_id || sdCapture.remote_path) && (
            <div className="estop-note" style={{ marginTop: 2 }}>
              {sdCapture.test_id ?? 'existing file'}
              {sdCapture.remote_path ? ` · ${sdCapture.remote_path}` : ''}
            </div>
          )}
          <div className="row" style={{ marginTop: 6 }}>
            <span className="text-muted">Test run</span>
            <span
              className={`mono ${
                run?.phase === 'motor_ready'
                  ? 'text-ok'
                  : run?.phase === 'failed' || run?.phase === 'interrupted'
                    ? 'text-error'
                    : ''
              }`}
            >
              {runLabel}
            </span>
          </div>
          {runHint && (
            <div className="estop-note" style={{ marginTop: 2 }}>
              {runHint}
            </div>
          )}
          {status.run.manifest_path && (
            <div className="estop-note" style={{ marginTop: 2 }}>
              Manifest · {status.run.manifest_path}
            </div>
          )}
          {(captureError || sdCapture.last_error) && (
            <div className="estop-note" role="alert" style={{ color: 'var(--mg-error)' }}>
              {captureError || captureErrorLabel(sdCapture.last_error)}
            </div>
          )}
          <div className="btn-row" style={{ marginTop: 4 }}>
            <button
              className="mode-btn"
              onClick={() => void captureCommand('check')}
              disabled={!status.target_connected || captureBusy || running || captureSettling}
            >
              Check logger
            </button>
            <button
              className="mode-btn"
              onClick={() => void startCapture()}
              disabled={
                !status.target_connected ||
                captureBusy ||
                running ||
                captureActive ||
                captureSettling
              }
            >
              Start capture
            </button>
            <button
              className="mode-btn"
              onClick={() => void captureCommand('stop')}
              disabled={!status.target_connected || captureBusy || running || !captureCanStop}
            >
              Stop capture
            </button>
          </div>
          <div className="estop-note" style={{ marginTop: 4 }}>
            SD capture is optional and controlled manually. START and STOP do
            not start or stop logging. Stop the motor test before stopping the
            capture or deleting SD logs.
          </div>

          <div className="row" style={{ marginTop: 8 }}>
            <span className="text-muted">SD logs</span>
            <span className="mono">{logEntries.length}</span>
          </div>
          <div className="btn-row" style={{ marginTop: 4 }}>
            <button
              className="mode-btn"
              onClick={() => void requestLogs()}
              disabled={!status.target_connected || logsBusy}
            >
              Refresh list
            </button>
            <button
              className="mode-btn"
              onClick={() => void eraseAllLogs()}
              disabled={
                !status.target_connected ||
                logsBusy ||
                logEntries.length === 0 ||
                running ||
                captureActive ||
                captureSettling
              }
              title="Delete every SD log after two confirmations"
            >
              Delete all…
            </button>
            {logsBusy && (
              <button className="mode-btn" onClick={() => void cancelLogs()}>
                Cancel
              </button>
            )}
          </div>
          {logsError && (
            <div className="estop-note" role="alert" style={{ color: 'var(--mg-error)' }}>
              {logsError}
            </div>
          )}
          <ul className="log-list" style={{ marginTop: 4 }}>
            {logEntries.map((entry) => {
              const dl = logDownloads.get(entry.id)
              const pct = dl
                ? Math.min(100, Math.round((dl.fetched / Math.max(1, dl.total)) * 100))
                : 0
              const date =
                entry.time_utc > 0
                  ? new Date(entry.time_utc * 1000).toISOString().slice(0, 16).replace('T', ' ')
                  : '—'
              return (
                <li key={entry.id} className="row" style={{ gap: 4 }}>
                  <span className="mono text-muted">#{entry.id}</span>
                  <span className="mono text-muted" style={{ flex: 1 }}>
                    {date}
                  </span>
                  <span className="mono">{(entry.size / 1024 / 1024).toFixed(1)} MB</span>
                  {dl?.path ? (
                    <span className="mono text-ok">saved</span>
                  ) : (
                    <button
                      className="mode-btn"
                      onClick={() => void downloadLog(entry)}
                      disabled={logsBusy || entry.size === 0 || captureActive || captureSettling}
                    >
                      Download
                    </button>
                  )}
                  <button
                    className="mode-btn"
                    onClick={() => void deleteLog(entry)}
                    disabled={
                      logsBusy || running || captureActive || captureSettling
                    }
                    title="Delete this SD log after two confirmations"
                  >
                    Delete
                  </button>
                  {dl && !dl.path && (
                    <span className="log-progress" style={{ marginLeft: 'auto' }}>
                      <span style={{ width: `${pct}%` }} />
                    </span>
                  )}
                </li>
              )
            })}
            {logEntries.length === 0 && !logsBusy && (
              <li className="estop-note">{logsListed ? 'No SD logs on the target.' : 'No logs listed yet — press Refresh list.'}</li>
            )}
          </ul>
          </div>
        </div>
        <div className="card">
          <div className="row">
            <span className="text-muted">Session</span>
            <span className="mono">{status.session}</span>
          </div>
        </div>

        {/* Mag alarm (A4b, plan §9.1): threshold watchdog over the calibrated
         * total field, with debounce + hysteresis + cooldown in core. */}
        <div className={`card ${magAlarm ? 'mag-alarm-active' : ''}`}>
          <h3>Mag alarm</h3>
          <label className="row" style={{ marginBottom: 6 }}>
            <span>Enabled</span>
            <input
              type="checkbox"
              checked={alarmEnabled}
              onChange={(e) => setAlarmEnabled(e.target.checked)}
            />
          </label>
          <label className="motor-row">
            <span className="motor-label" style={{ width: 52 }}>Thr.</span>
            <input
              className="phrase-input"
              style={{ flex: 1, margin: 0 }}
              type="number"
              min={0}
              step={100}
              value={alarmThreshold}
              onChange={(e) => setAlarmThreshold(Number(e.target.value))}
            />
            <span className="mono motor-value">nT</span>
          </label>
          <div className="row">
            <span className="text-muted">Total field</span>
            <span className="mono">{magTotal != null ? `${magTotal.toFixed(1)} nT` : '—'}</span>
          </div>
          <div className="row">
            <span className="text-muted">State</span>
            <span className={`mono ${magAlarm ? 'text-error' : 'text-ok'}`}>
              {magAlarm ? 'ALARM' : 'armed'}
            </span>
          </div>
          {tauri && (
            <div className="row" style={{ marginTop: 6 }}>
              <span className="text-muted">Calibration</span>
              {calLoaded ? (
                <>
                  <span className="mono text-ok">CAL</span>
                  <button className="mode-btn" onClick={clearMagCal}>
                    Clear
                  </button>
                </>
              ) : (
                <button className="mode-btn" onClick={loadMagCal}>
                  Load…
                </button>
              )}
            </div>
          )}
          {magAlarm && (
            <button
              className="mode-btn"
              style={{ width: '100%', marginTop: 6 }}
              onClick={acknowledgeAlarm}
            >
              Acknowledge (silence)
            </button>
          )}
        </div>

        <MotTestEnGate
          motTestEn={
            status.com_mot_test_en != null
              ? status.com_mot_test_en !== 0
              : motTestEn
          }
        />

        <div className="estop-note">
          Tests run while the FC is disarmed; PX4 denies ACTUATOR_TEST otherwise.
          Enable COM_MOT_TEST_EN=1 in the target Parameters window if needed.
        </div>
      </aside>
    </div>
  )
}
/** PM9 gate: when the parameter cache says COM_MOT_TEST_EN=0, motor tests
 * are FC-denied anyway — say so up front and offer the jump to the
 * Parameters window (open-or-focus, same entry the TopBar uses). Unknown
 * (never pulled) renders nothing: the estop note below covers it. */
function MotTestEnGate({ motTestEn }: { motTestEn: boolean | null }) {
  if (motTestEn !== false) return null
  const openParams = () => {
    void invoke('params_open').catch(() => {})
  }
  return (
    <div className="mot-en-gate">
      <span>
        COM_MOT_TEST_EN=0 — the FC rejects every motor-test command. Automatic
        enable did not take; set it to 1 in Parameters (reboot after Apply).
      </span>
      <button className="mode-btn" style={{ width: '100%', marginTop: 6 }} onClick={openParams}>
        Open Parameters
      </button>
    </div>
  )
}
