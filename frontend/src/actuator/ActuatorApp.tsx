// Motor / mag interference test window — A1+A3 (motor-test §5–§6, §8.2),
// refactored to a source-driven layout (operator request 2026-10-10):
//
//   Header: target identity, test state, Stop and Emergency Stop
//   Left: SITL input, unlock/start controls and magnetic alarm
//   Mid: safety/source settings, motor comparison and realtime curves
//   Right: serial target and recording/log retrieval
//
// One primary action per state: START (3 s hold) dispatches by source —
// manual sliders or a preset waveform; STOP while running. The gate
// (unlock → hold) applies to every source, across the full configured output range.

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { serialPortForEndpoint } from '../stores/devices'
import { useSerialSelection } from '../hooks/useSerialSelection'
import { listen } from '@tauri-apps/api/event'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { readTextFile } from '@tauri-apps/plugin-fs'
import type { LogEntryInfo } from '../generated-types/LogEntryInfo'
import { isTauri } from '../inspector/mock'
import BenchHeader, { type BenchFailure } from './BenchHeader'
import {
  IDLE,
  IDLE_TONES,
  MOTOR_COUNT,
  activeProtocolSummary,
  activeMotorCount,
  channelLabel,
  isDShot,
  outputUnits,
  mappingHasGap,
  motorGuide,
  outputConfigurationIssue,
  protocolLabel,
  type ActuatorStatus,
  type RunStatus,
  type SdCaptureError,
  type SdCaptureStatus,
  type ServoPort,
} from './benchTypes'
import MotorGuideCard from './MotorGuide'
import MotorTrackingTable from './MotorTrackingTable'
import SignalScope, { useScopeFeed } from './SignalScope'
import StartStopButton from './StartStopButton'
import MotorToneControls from './MotorToneControls'
import StreamManager from './StreamManager'
import { scopeStore } from './scopeStore'

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

interface LoggingAttentionInput {
  captureBusy: boolean
  captureActive: boolean
  captureSettling: boolean
  captureFailed: boolean
  captureError: boolean
  logsError: boolean
}

export function loggingNeedsAttention(input: LoggingAttentionInput): boolean {
  return input.captureBusy
    || input.captureActive
    || input.captureSettling
    || input.captureFailed
    || input.captureError
    || input.logsError
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

/** Hold duration required before starting a motor test. */
const PROPS_HOLD_MS = 3000

/** Slider updates are merged at ~30 Hz — never queued (plan §8.1). */
const SET_VALUES_MIN_INTERVAL_MS = 33

/** One Hz status poll; safety-critical signals (armed!) are low-rate. */
const STATUS_POLL_MS = 1000

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
  /** Routing is frozen while the motor test is sending. */
  locked: boolean
  error: string | null
  highlightConnect?: boolean
  onDismissError: () => void
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
        disabled={props.connected || props.locked}
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
          disabled={props.connected || props.locked}
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
          disabled={props.connected || props.locked}
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
          disabled={props.connected || props.locked}
        >
          <option value="main">MAIN</option>
          <option value="aux">AUX</option>
        </select>
      </label>
      {props.error && (
        <InlineError message={props.error} onDismiss={props.onDismissError} />
      )}
      {props.connected ? (
        <button
          className="mode-btn"
          style={{ width: '100%', marginTop: 6 }}
          onClick={props.onDisconnect}
          disabled={props.locked}
          title={props.locked ? 'Stop the motor test before changing the source' : undefined}
        >
          Disconnect SITL
        </button>
      ) : (
        <button
          className={`mode-btn${props.highlightConnect ? ' guide-target' : ''}`}
          style={{ width: '100%', marginTop: 6 }}
          onClick={props.onConnect}
          disabled={!validIds || props.endpoint.trim() === '' || props.locked}
          title={props.locked ? 'Stop the motor test before changing the source' : undefined}
        >
          Connect SITL
        </button>
      )}
    </>
  )
}

/** Secondary card section: collapsed by default, opened by a warning, and
 * kept open once the operator opens it (a warning never closes it). */
function FoldSection({
  title,
  defaultOpen = false,
  forceOpen = false,
  children,
}: {
  title: string
  defaultOpen?: boolean
  forceOpen?: boolean
  children: ReactNode
}) {
  const [open, setOpen] = useState(defaultOpen)
  const visible = forceOpen || open
  return (
    <section className="fold-section">
      <button
        type="button"
        className="fold-head"
        aria-expanded={visible}
        disabled={forceOpen}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="section-title">{title}</span>
        <span className="fold-state mono">
          {forceOpen ? 'required' : visible ? 'fold' : 'unfold'}
        </span>
      </button>
      <div hidden={!visible}>{children}</div>
    </section>
  )
}

export function ActuatorApp() {
  const tauri = isTauri()
  // Collect scope samples for the window's whole lifetime, so the history
  // is there when the operator looks at the scope below the controls.
  useScopeFeed()
  const [status, setStatus] = useState<ActuatorStatus>(IDLE)
  const [source, setSource] = useState<Source>('manual')
  /** Ordinary STOP retains the local unlock. Idle command edits and emergency
   * reset re-lock it; every start still requires the full 3 s hold. */
  const [unlocked, setUnlocked] = useState(false)
  /** Latched emergency: stays until an explicit reset, never auto-clears. */
  const [emergency, setEmergency] = useState(false)
  /** Backend refusal or transport failure. Retained in the header until the
   * operator dismisses it or the same operation succeeds again. */
  const [failure, setFailure] = useState<BenchFailure | null>(null)
  /** A start command is in flight (backend readiness/transport). */
  const [starting, setStarting] = useState(false)
  const [tonesReady, setTonesReady] = useState(false)
  const [controlBusy, setControlBusy] = useState(true)
  const [controlError, setControlError] = useState<string | null>(null)
  const controlEnabled = status.motor_control?.enabled ?? false
  const setMotorControl = useCallback(async (enabled: boolean) => {
    setControlBusy(true)
    setControlError(null)
    setUnlocked(false)
    if (!enabled) setStatus((current) => ({ ...current, motor_control: { enabled: false } }))
    try {
      const control = tauri
        ? await invoke<{ enabled: boolean }>('actuator_set_motor_control', { enabled })
        : { enabled }
      setStatus((current) => ({ ...current, motor_control: control }))
    } catch (error) {
      setControlError(String(error))
    } finally { setControlBusy(false) }
  }, [tauri])
  useEffect(() => { void setMotorControl(false) }, [setMotorControl])
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
  // The selected FC: a dedicated serial PX4 or the borrowed main serial link.
  const [targetEndpoint, setTargetEndpoint] = useState('serial:/dev/ttyACM0:115200')
  const targetSerialSelection = useSerialSelection(targetEndpoint)
  const sitlSerialSelection = useSerialSelection(sitlEndpoint)
  const [targetConnectError, setTargetConnectError] = useState<string | null>(null)
  // Why the last session stopped (armed / heartbeat_stale / acks_denied / …).
  const [stopReason, setStopReason] = useState<string | null>(null)
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
  const [captureErrorDismissed, setCaptureErrorDismissed] = useState(false)
  // SD-log list + download (feature 2): `actuator_logs` events.
  const [logEntries, setLogEntries] = useState<LogEntryInfo[]>([])
  const [logsListed, setLogsListed] = useState(false)
  const [logDownloads, setLogDownloads] = useState<Map<number, { fetched: number; total: number; path?: string }>>(new Map())
  const [logsBusy, setLogsBusy] = useState(false)
  const [logsError, setLogsError] = useState<string | null>(null)
  // Presentation-only disclosure state: startup controls and recording actions
  // stay visible while optional input and retrieval details can be expanded.
  const [sitlExpanded, setSitlExpanded] = useState(false)
  const [legendOpen, setLegendOpen] = useState(true)
  const [loggingDrawerOpen, setLoggingDrawerOpen] = useState(false)
  const [loggingExpanded, setLoggingExpanded] = useState(false)
  const audioCtxRef = useRef<AudioContext | null>(null)
  const beepTimer = useRef<number | undefined>(undefined)

  const valuesRef = useRef(values)
  valuesRef.current = values
  const lastSend = useRef(0)

  const running = status.session === 'running' || status.session === 'stopping'
  const releasing = status.session === 'stopping'
  const pendingStart = starting || Boolean(status.tones?.starting)
  // Only the configured, contiguous M1..MN prefix is a motor bench. Empty
  // trailing slots and mapping gaps never become interactive controls.
  const activeMotors = activeMotorCount(status.output_map)
  const activeModels = status.pwm_models.slice(0, activeMotors)
  const mapGap = mappingHasGap(status.output_map)

  useEffect(() => {
    setSitlExpanded(source === 'forward')
  }, [source])

  // Emergency stop: one gesture, always available. `Esc` is captured on the
  // window in the capture phase, so it works while focus is inside a slider
  // or text input; Space is deliberately NOT bound (plan 2.2-4).
  const estop = useCallback(() => {
    setEmergency(true)
    if (!tauri) return
    void invoke('actuator_estop').catch((error: unknown) => {
      setFailure({ operation: 'emergency_stop', message: String(error) })
    })
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
        const configured = scopeStore.configureOutputs(s.scope_outputs)
        if (!configured.ok) return
        setStatus(s)
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
    setCaptureErrorDismissed(false)
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
      void invoke('actuator_set_values', {
        values: next.slice(0, activeMotors),
      }).catch(() => {})
    },
    [tauri, running, activeMotors],
  )

  const setMotor = (index: number, v: number) => {
    const next = sync ? values.map(() => v) : values.map((x, i) => (i === index ? v : x))
    setValues(next)
    pushValues(next)
    if (!running) resetConfirmation()
  }

  /** Any idle change to what will be commanded invalidates the start cover. */
  const resetConfirmation = () => {
    setUnlocked(false)
  }

  const changeSource = (next: Source) => {
    setSource(next)
    resetConfirmation()
  }

  const changeSitlEndpoint = (value: string) => {
    setSitlEndpoint(value)
    resetConfirmation()
  }
  const changeSitlSystemId = (value: number) => {
    setSitlSystemId(value)
    resetConfirmation()
  }
  const changeSitlComponentId = (value: number) => {
    setSitlComponentId(value)
    resetConfirmation()
  }
  const changeSitlPort = (value: ServoPort) => {
    setSitlPort(value)
    resetConfirmation()
  }
  const changeTargetEndpoint = (value: string) => {
    setTargetEndpoint(value)
    resetConfirmation()
  }

  const resetEmergency = () => {
    setEmergency(false)
    resetConfirmation()
    if (tauri) void invoke('actuator_reset_emergency').catch(() => {})
  }

  const stop = async () => {
    if (!tauri) return
    try {
      await invoke('actuator_stop')
      setFailure(null)
    } catch (error) {
      setFailure({ operation: 'stop', message: String(error) })
    }
  }

  /** What still gates Start: unlock → 3 s hold. No prop-load choice. */
  const gate = useMemo(() => {
    if (!controlEnabled) return 'control_off' as const
    if (emergency) return 'emergency' as const
    if (!unlocked) return 'unlock' as const
    return 'ready' as const
  }, [controlEnabled, emergency, unlocked])

  /** The one start action — dispatches by the selected source. */
  const start = async () => {
    if (!tauri || gate !== 'ready' || running) return
    setFailure(null)
    // The backend refuses (e.g. "no link — connect the serial PX4 first");
    // surface it instead of silently doing nothing.
    setStopReason(null)
    setStarting(true)
    try {
      if (source === 'manual') {
        await invoke('actuator_start_manual', {
          values: valuesRef.current.slice(0, activeMotors),
        })
      } else if (source === 'preset') {
        await invoke('actuator_start_preset', {
        preset: {
          kind: preset,
          amplitude,
          frequency_hz: frequency,
          duration_s: duration,
        },
        })
      } else if (source === 'forward') {
        await invoke('actuator_start_forward')
      }
    } catch (error) {
      if (String(error).includes('motor start/tone cancelled')) return
      setFailure({
        operation: 'start',
        message: typeof error === 'string' ? error : String(error),
      })
    } finally {
      setStarting(false)
    }
  }

  const connectSitl = () => {
    if (!tauri) return
    resetConfirmation()
    setSitlConnectError(null)
    if (sitlSerialSelection.changed) { setSitlConnectError('The serial attachment changed. Re-enter its endpoint.'); return }
    void (sitlSerialSelection.port ? Promise.resolve(sitlSerialSelection.port) : serialPortForEndpoint(sitlEndpoint)).then((expectedSerial) => invoke('actuator_connect_sitl', {
      expectedSerial,
      source: {
        endpoint: sitlEndpoint,
        system_id: sitlSystemId,
        component_id: sitlComponentId,
        port: sitlPort,
      },
    })).catch((error: unknown) => {
      setSitlConnectError(String(error))
    })
  }
  const disconnectSitl = () => {
    if (!tauri) return
    resetConfirmation()
    setSitlConnectError(null)
    void invoke('actuator_disconnect_sitl').catch((error: unknown) => {
      setSitlConnectError(String(error))
    })
  }
  const connectTarget = () => {
    if (!tauri) return
    resetConfirmation()
    setTargetConnectError(null)
    if (targetSerialSelection.changed) { setTargetConnectError('The serial attachment changed. Re-enter its endpoint.'); return }
    void (targetSerialSelection.port ? Promise.resolve(targetSerialSelection.port) : serialPortForEndpoint(targetEndpoint)).then((expectedSerial) => invoke('actuator_connect_target', {
      expectedSerial,
      endpoint: targetEndpoint,
    })).catch((error: unknown) => {
      setTargetConnectError(String(error))
    })
  }
  const disconnectTarget = () => {
    if (!tauri) return
    resetConfirmation()
    void invoke('actuator_disconnect_target').catch((error: unknown) => {
      setTargetConnectError(String(error))
    })
  }
  const useMainTarget = () => {
    if (!tauri || !status.main_serial_target) return
    resetConfirmation()
    setTargetConnectError(null)
    void invoke('actuator_use_main_target', { expected: status.main_serial_target }).catch((error: unknown) => {
      setTargetConnectError(String(error))
    })
  }
  const refreshMapping = () => {
    if (!tauri) return
    resetConfirmation()
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
  // Only the active prefix can block a start: its range must be FC-read and
  // its PX4 output protocol must be known.
  const fallbackMotors = activeModels
    .map((model, index) => (model.range_source !== 'fc' ? index + 1 : 0))
    .filter((motor) => motor !== 0)
  const unknownProtocolMotors = activeModels
    .map((model, index) => (model.protocol.kind === 'unknown' ? index + 1 : 0))
    .filter((motor) => motor !== 0)
  const dshotActive = activeModels.some((model) => isDShot(model.protocol))
  const dshot3dBlocked = dshotActive && status.dshot_3d_enable !== 0
  const rpmLive = status.esc_rpm.filter((row) => row.rpm != null && !row.stale).length
  const noActiveMotors = status.target_connected && activeMotors === 0
  const missingModels =
    status.target_connected && status.pwm_models.length < activeMotors
  const outputConfigurationBlocked =
    noActiveMotors ||
    missingModels ||
    mapGap ||
    fallbackMotors.length > 0 ||
    unknownProtocolMotors.length > 0 ||
    dshot3dBlocked
  const outputSummary = activeProtocolSummary(status)
  const outputIssue = outputConfigurationIssue(status)
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
  const guide = motorGuide({
    targetConnected: status.target_connected,
    targetFcAlive: status.target_fc_alive,
    armed: status.armed,
    emergency,
    running,
    pendingStart,
    outputBlocked: outputConfigurationBlocked,
    outputIssue,
    controlEnabled,
    unlocked,
    runBlocksStart,
    tonesReady,
    sourceNeedsSitl: source === 'forward' && !status.sitl_connected,
  })
  const loggingAttention = loggingNeedsAttention({
    captureBusy,
    captureActive,
    captureSettling,
    captureFailed: sdCapture.phase === 'failed',
    captureError:
      captureError != null || (sdCapture.last_error != null && !captureErrorDismissed),
    logsError: logsError != null,
  })

  useEffect(() => {
    if (loggingAttention) setLoggingDrawerOpen(true)
  }, [loggingAttention])

  return (
    <div className={`actuator-grid ${magAlarm && alarmEnabled ? 'mag-alarm-flash' : ''}`}>
      <BenchHeader
        status={status}
        running={running}
        starting={pendingStart}
        releasing={releasing}
        emergency={emergency}
        unlocked={unlocked}
        stopReason={stopReason}
        failure={failure}
        guideAction={guide.action}
        onStop={stop}
        onEmergencyStop={estop}
        onResetEmergency={resetEmergency}
        onDismissFailure={() => setFailure(null)}
      />

      {/* Left: input context, deliberate start controls and magnetic monitoring. */}
      <aside className="actuator-left" aria-label="Test controls and monitoring">
        <div className="section-title">Controls & monitoring</div>
        <div className="card sitl-card">
          <div className="card-heading">
            <h3>SITL source</h3>
            <button
              className="link-btn"
              aria-label="SITL source details"
              aria-expanded={sitlExpanded}
              aria-controls="motor-sitl-details"
              onClick={() => setSitlExpanded((v) => !v)}
            >
              {sitlExpanded ? 'Hide' : 'Show'}
            </button>
          </div>
          <div className="row source-summary">
            <span className="estop-note">Input for Forward mode</span>
            <span className={`mono ${status.sitl_connected ? 'text-ok' : 'text-muted'}`}>
              {status.sitl_connected ? 'connected' : 'offline'}
            </span>
          </div>
          <div id="motor-sitl-details" hidden={!sitlExpanded}>
          <SitlSourceFields
            endpoint={sitlEndpoint}
            onEndpoint={changeSitlEndpoint}
            systemId={sitlSystemId}
            onSystemId={changeSitlSystemId}
            componentId={sitlComponentId}
            onComponentId={changeSitlComponentId}
            port={sitlPort}
            onPort={changeSitlPort}
            connected={status.sitl_connected}
            locked={running || pendingStart}
            error={sitlConnectError}
            highlightConnect={guide.action === 'sitl'}
            onDismissError={() => setSitlConnectError(null)}
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
            {Array.from({ length: activeMotors }, (_, i) => {
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
        </div>

        <div className="card test-control-card" data-testid="motor-test-controls">
          <h3>Test controls</h3>
          <button
            className={`mode-btn motor-control-switch ${controlEnabled ? 'active' : ''}${guide.action === 'enable' ? ' guide-target' : ''}`}
            role="switch"
            aria-label="Motor control"
            aria-checked={controlEnabled}
            disabled={controlBusy || (!controlEnabled && (!status.target_connected || emergency || releasing))}
            title={controlEnabled
              ? 'Turn motor control OFF: cancel pending starts and release only outputs owned by this Motor Test session.'
              : !status.target_connected
                ? 'Connect a PX4 target first. OFF is monitor-only and sends no motor test or ESC tone commands.'
                : 'Enable Motor Test commands for this connection. This does not start motors; unlock and hold START separately.'}
            onClick={() => { void setMotorControl(!controlEnabled) }}
          >Motor control <strong>{controlEnabled ? 'ON' : 'OFF'}</strong></button>
          {!controlEnabled && <div className="estop-note" data-testid="motor-control-off">Monitor only · no motor or ESC tone commands</div>}
          {controlError && <div className="estop-note text-error" role="alert">{controlError}</div>}
          <button
            className={`mode-btn unlock-btn${guide.action === 'unlock' ? ' guide-target' : ''}`}
            onClick={() => (emergency ? undefined : setUnlocked((u) => !u))}
            disabled={!controlEnabled || controlBusy || emergency || running || pendingStart}
            aria-pressed={unlocked}
            title={!controlEnabled ? 'Motor control is OFF. Enable it before unlocking START.' : unlocked ? 'Re-lock START without changing the flight controller arming state.' : 'Unlock this test. Motors stay stopped until the full START hold completes.'}
          >
            {unlocked ? 'Re-lock test' : 'Unlock test'}
          </button>
          <StartStopButton
            running={running}
            pending={pendingStart}
            holdMs={PROPS_HOLD_MS}
            highlight={guide.action === 'start' || guide.action === 'cancel'}
            onStart={start}
            onStop={stop}
            disabled={
              gate !== 'ready' ||
              controlBusy ||
              !tonesReady ||
              (source === 'forward' && !status.sitl_connected) ||
              !status.target_connected ||
              !status.target_fc_alive ||
              status.armed ||
              runBlocksStart ||
              outputConfigurationBlocked
            }
            title={
              gate === 'control_off'
                ? 'Motor control is OFF. Enable it to allow this tool to command motors.'
                : gate === 'emergency'
                ? 'Reset the emergency latch first'
                : gate === 'unlock'
                  ? 'Unlock the test first'
                  : !status.target_connected
                      ? 'Connect the serial PX4 first'
                      : !status.target_fc_alive
                        ? 'Waiting for the target PX4 heartbeat'
                        : status.armed
                          ? 'Disarm the target PX4 first'
                          : noActiveMotors
                            ? 'No configured motor outputs — assign PWM_MAIN/AUX_FUNCn and refresh'
                            : missingModels
                              ? 'Output models are not loaded — refresh the mapping'
                              : mapGap
                              ? 'Output mapping has a gap — fix PWM_MAIN/AUX_FUNCn and refresh'
                              : fallbackMotors.length > 0
                                ? `Output range not read from the PX4 for M${fallbackMotors.join(', M')} — refresh the mapping first`
                                : unknownProtocolMotors.length > 0
                                  ? `Output protocol unknown for M${unknownProtocolMotors.join(', M')} — check PWM_MAIN/AUX_TIMn`
                                  : dshot3dBlocked
                                    ? 'DSHOT_3D_ENABLE is not 0 — 3D mode changes the throttle range'
                                    : runBlocksStart
                            ? runHint ?? 'Wait for the active test run'
                            : source === 'forward' && !status.sitl_connected
                              ? 'Connect the SITL source first'
                              : 'Hold for 3 seconds'
            }
          />
          <div className="estop-note" style={{ marginTop: 6 }}>
            {running
              ? 'Sending — Esc or the red button stops everything.'
              : !controlEnabled ? 'Telemetry stays available. Enable motor control only when you intend to test.' : 'No arming required: the test runs while the FC is disarmed.'}
          </div>
        </div>

        <MotorToneControls
          tauri={tauri}
          supported={activeModels.some((model) => isDShot(model.protocol))}
          locked={running || pendingStart || emergency}
          status={status.tones ?? IDLE_TONES}
          onReady={setTonesReady}
          onChange={resetConfirmation}
        />

        {/* The alarm observes the total field; its watchdog remains in core. */}
        <div className={`card mag-alarm-card ${magAlarm && alarmEnabled ? 'mag-alarm-active' : ''}`} data-testid="motor-mag-alarm">
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
            <span className="motor-label" style={{ width: 64 }}>Threshold</span>
            <input
              className="phrase-input"
              style={{ flex: 1, margin: 0 }}
              type="number"
              min={0}
              step={100}
              value={alarmThreshold}
              onChange={(e) => setAlarmThreshold(Number(e.target.value))}
              aria-label="Magnetic alarm threshold (nT)"
            />
            <span className="mono motor-value">nT</span>
          </label>
          <div className="row">
            <span className="text-muted">Total field</span>
            <span className="mono">{magTotal != null ? `${magTotal.toFixed(1)} nT` : '—'}</span>
          </div>
          <div className="row">
            <span className="text-muted">State</span>
            <span
              className={`mono ${!alarmEnabled ? 'text-muted' : magAlarm ? 'text-error' : magTotal == null ? 'text-muted' : 'text-ok'}`}
              data-testid="motor-mag-state"
              role="status"
            >
              {!alarmEnabled ? 'Disabled' : magAlarm ? 'ALARM' : magTotal == null ? 'No data' : 'Monitoring'}
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
      </aside>

      {/* Middle: command setup, motor feedback and the flexible plot workspace. */}
      <main className="actuator-mid">
        <MotorGuideCard guide={guide} />
        <div className="controls-grid">
          <div className="card">
            <h3>Safety</h3>
            <div className="estop-note">
              Motor demand uses the full PX4-configured output range (0–100%),
              with 2.0/s slew and a 3 s START hold.
            </div>
          </div>

          <div className="card">
            <h3>Command source</h3>
            <div className="btn-row">
              {SOURCES.map((s) => (
                <button
                  key={s.id}
                  className={`mode-btn ${source === s.id ? 'active' : ''}`}
                  disabled={running || pendingStart}
                  onClick={() => changeSource(s.id)}
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
          <div className="card motor-controls-card">
            <h3>Motors</h3>
            <label className="row" style={{ marginBottom: 6 }}>
              <span>Synchronize all</span>
              <input
                type="checkbox"
                checked={sync}
                disabled={!controlEnabled || emergency || pendingStart}
                onChange={(e) => setSync(e.target.checked)}
              />
            </label>
            {activeMotors === 0 ? (
              <div className="estop-note text-warn">
                No configured motor outputs yet. Connect the serial PX4 and
                refresh the mapping.
              </div>
            ) : (
              <div className="motor-control-grid">
                {values.slice(0, activeMotors).map((v, i) => (
                  <label key={i} className="motor-row motor-control-row">
                    <span className="mono motor-label">M{i + 1}</span>
                    <input
                      type="range"
                      min={0}
                      max={1}
                      step={0.01}
                      value={v}
                      disabled={!controlEnabled || emergency || pendingStart}
                      onChange={(e) => setMotor(i, Number(e.target.value))}
                    />
                    <span className="mono motor-value">{(v * 100).toFixed(0)}%</span>
                  </label>
                ))}
              </div>
            )}
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
                  disabled={!controlEnabled || emergency || pendingStart}
                  onClick={() => {
                    setPreset(p.id)
                    resetConfirmation()
                  }}
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
                disabled={!controlEnabled || emergency || pendingStart}
                onChange={(e) => {
                  setAmplitude(Number(e.target.value))
                  resetConfirmation()
                }}
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
                disabled={!controlEnabled || emergency || pendingStart}
                onChange={(e) => {
                  setFrequency(Number(e.target.value))
                  resetConfirmation()
                }}
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
                disabled={!controlEnabled || emergency || pendingStart}
                onChange={(e) => {
                  setDuration(Number(e.target.value))
                  resetConfirmation()
                }}
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
            <div className="row">
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
              Configure the source once in the Source card; values pass the
              same interlocks (slew, cap, disarmed-only) as manual control.
            </div>
          </div>
        )}

      </div>
      <MotorTrackingTable
        status={status}
        statusFresh={status.target_connected && status.target_fc_alive}
        source={source}
        values={values}
        running={running}
      />
      <SignalScope
        tools={<StreamManager tauri={tauri} />}
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
        <div className="card target-card">
          <div className="card-heading">
            <h3>PX4 target</h3>
            <span className={`chip ${status.target_connected ? 'chip-ok' : 'chip-warn'}`}>
              {status.target_connected ? 'connected' : 'offline'}
            </span>
          </div>
          {status.target_connected ? (
            <div className="target-endpoint mono" title={status.target_endpoint ?? targetEndpoint}>
              {status.target_origin === 'main_serial' && <div className="text-ok" data-testid="main-serial-attached">Using main serial link</div>}
              {status.target_endpoint ?? targetEndpoint}
            </div>
          ) : (
            <div className="target-connect-fields">
              {status.main_serial_target && <div className="card">
                <div className="estop-note">Main connection · PX4 {status.main_serial_target.target.target_system_id}</div>
                <div className="target-endpoint mono">{status.main_serial_target.endpoint}</div>
                <button className="mode-btn target-link-action" type="button" onClick={useMainTarget} disabled={running || pendingStart}
                  title="Attach to the main window's existing serial link. Motor control starts OFF; detaching keeps the main connection open.">
                  Use main serial link
                </button>
              </div>}
              <input
                className="phrase-input"
                value={targetEndpoint}
                onChange={(e) => changeTargetEndpoint(e.target.value)}
                placeholder="serial:/dev/ttyACM0:115200"
                aria-label="Target PX4 serial endpoint"
                spellCheck={false}
                disabled={running || pendingStart}
              />
              <div className="estop-note">
                MagGCS identifies the PX4 on this port automatically and keeps its
                own MAVLink ID separate.
              </div>
            </div>
          )}
          {targetConnectError && (
            <InlineError
              message={targetConnectError}
              onDismiss={() => setTargetConnectError(null)}
            />
          )}
          {status.target_connected ? (
            <button
              className="mode-btn target-link-action"
              onClick={disconnectTarget}
              title={running || pendingStart ? 'Stop or cancel the motor test before changing its target link.' : status.target_origin === 'main_serial' ? 'Detach Motor Test after releasing its outputs; keep the main serial connection open.' : 'Detach the Motor Test target after releasing this tool’s outputs.'}
              disabled={running || pendingStart}
            >
              {status.target_origin === 'main_serial' ? 'Detach main link' : 'Disconnect'}
            </button>
          ) : (
            <button
              className={`mode-btn target-link-action${guide.action === 'connect' ? ' guide-target' : ''}`}
              onClick={connectTarget}
              title="Connect the selected target link for telemetry. Motor control starts OFF."
              disabled={running || pendingStart}
            >
              Connect PX4
            </button>
          )}
          <div className="row target-summary-row">
            <span className="text-muted">FC</span>
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
                  ? status.armed
                    ? 'alive · ARMED'
                    : 'alive · disarmed'
                  : 'lost'}
            </span>
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
          <div className="row target-summary-row">
            <span className="text-muted">Output</span>
            <span
              className={`mono ${
                outputIssue == null
                  ? 'text-ok'
                  : 'text-warn'
              }`}
            >
              {status.output_map.state} · {outputSummary}
            </span>
          </div>
          {outputIssue && <div className="estop-note text-warn">{outputIssue}</div>}
          <FoldSection
            title="Connection & output details"
            forceOpen={outputConfigurationBlocked || status.output_map.issues.length > 0}
          >
            <div className="row target-detail-first">
              <span className="text-muted">System ID / mode</span>
              <span className="mono">
                {status.target_system_id ?? '—'} · {status.autopilot_px4 && status.mode_label ? status.mode_label : '—'}
              </span>
            </div>
            <div className="row">
              <span className="text-muted">PX4 banks</span>
              <span className="protocol-pair mono">
                <span className="protocol-chip">
                  MAIN {protocolLabel(status.output_protocols.main)}
                </span>
                <span className="protocol-chip">
                  AUX {protocolLabel(status.output_protocols.aux)}
                </span>
              </span>
            </div>
            {dshotActive && <div className="row">
              <span className="text-muted">DShot 3D / bidir</span>
              <span className={`mono ${dshot3dBlocked ? 'text-error' : ''}`}>
                {status.dshot_3d_enable == null
                  ? '3D unread'
                  : status.dshot_3d_enable === 0
                    ? 'off'
                    : `on (${status.dshot_3d_enable})`}
                {' / '}
                {status.dshot_bidir_en == null
                  ? 'bidir unread'
                  : status.dshot_bidir_en === 0
                    ? 'bidir off'
                  : 'bidir on'}
              </span>
            </div>}
            <div className="target-output-list">
              <div className="estop-note">
                {status.output_map.motors.length === 0
                  ? 'Outputs not read'
                  : status.output_map.motors
                      .slice(0, activeMotors)
                      .map((assignment) => assignment.channel
                        ? `M${assignment.motor} ${channelLabel(assignment.channel)}`
                        : `M${assignment.motor} unmapped`)
                      .join(' · ')}
              </div>
              {activeModels.length > 0 && (
                <div className="estop-note">
                  {activeModels.map((model, i) => {
                    const units = outputUnits(model)
                    const range = units === 'raw' ? 'unit unknown' : `${model.min_us}–${model.max_us} ${units}`
                    return `M${i + 1} ${range}${model.reversed ? ' rev' : ''}`
                  }).join(' · ')}
                </div>
              )}
            </div>
            {status.output_map.issues.length > 0 && (
              <div className="estop-note text-error">
                {status.output_map.issues.length} output assignment issue
                {status.output_map.issues.length === 1 ? '' : 's'}
              </div>
            )}
            <button
              className={`mode-btn target-refresh${guide.action === 'refresh' ? ' guide-target' : ''}`}
              onClick={refreshMapping}
              disabled={!status.target_connected || running || pendingStart}
            >
              Refresh output configuration
            </button>
          </FoldSection>
          {mappingError && (
            <InlineError
              message={mappingError}
              onDismiss={() => setMappingError(null)}
            />
          )}
          <MotTestEnGate
            motTestEn={
              status.com_mot_test_en != null
                ? status.com_mot_test_en !== 0
                : null
            }
            mainSerial={status.target_origin === 'main_serial'}
          />
        </div>
        {/* Display-only ESC telemetry, indexed independently of motor outputs. */}
        <div className="card rpm-card">
          <h3>ESC RPM feedback</h3>
          <div className="row">
            <span className="text-muted">ESC_STATUS</span>
            <span className={`mono ${rpmLive > 0 ? 'text-ok' : ''}`}>
              {status.esc_rpm.length === 0
                ? 'no telemetry'
                : rpmLive > 0
                  ? `${rpmLive} live`
                  : 'stale / missing'}
            </span>
          </div>
          <div className="row">
            <span className="text-muted">DSHOT_BIDIR_EN</span>
            <span className={`mono ${status.dshot_bidir_en === 1 ? 'text-ok' : 'text-warn'}`}>
              {status.dshot_bidir_en == null
                ? 'unread'
                : status.dshot_bidir_en === 0
                  ? 'off'
                  : status.dshot_bidir_en === 1
                    ? 'enabled'
                    : `on (${status.dshot_bidir_en})`}
            </span>
          </div>
          {status.esc_rpm.length > 0 && (
            <div className="rpm-grid">
              {status.esc_rpm.map((row) => (
                <span
                  key={row.esc}
                  className={`rpm-chip ${
                    row.rpm == null ? 'is-missing' : row.stale ? 'is-stale' : 'is-live'
                  }`}
                  title={
                    row.age_s == null
                      ? 'this ESC has not reported'
                      : `${row.age_s.toFixed(1)} s old`
                  }
                >
                  <b>ESC{row.esc}</b>
                  {row.rpm == null ? ' —' : ` ${Math.round(row.rpm)}`}
                  {row.stale ? ' stale' : ''}
                </span>
              ))}
            </div>
          )}
          <div className="estop-note" style={{ marginTop: 4 }}>
            Display-only telemetry. ESC indices are not motor assignments.
          </div>
          {dshotActive && status.dshot_bidir_en !== 1 && (
            <div className="estop-note text-warn" style={{ marginTop: 4 }}>
              Bidirectional DShot RPM needs DSHOT_BIDIR_EN=1. Other ESC telemetry
              depends on what the connected PX4 exposes.
            </div>
          )}
        </div>
        {/* On-board SD-card log: read the restart-required SD parameters and
         * control the target's file logger; list/download the FC's SD logs. */}
        <div className={`card logging-card logging-drawer ${loggingDrawerOpen ? 'is-open' : ''}`}>
          <div className="logging-drawer-bar">
            <button
              className="logging-drawer-toggle"
              aria-expanded={loggingDrawerOpen}
              aria-controls="motor-logging-drawer"
              onClick={() => setLoggingDrawerOpen((open) => !open)}
            >
              <span>Capture & logs</span>
              <span className="mono text-muted">
                {sdCapture.phase.replaceAll('_', ' ')} · {logEntries.length} files
                <span aria-hidden="true"> {loggingDrawerOpen ? '▾' : '▴'}</span>
              </span>
            </button>
            <div className="logging-drawer-actions">
              <button
                className="mode-btn"
                onClick={() => void startCapture()}
                title={!status.target_connected ? 'Connect the PX4 target first.' : captureBusy ? 'Wait for the current capture request.' : 'Start a new PX4 SD recording. This is independent of motor START and motor control.'}
                disabled={
                  !status.target_connected ||
                  captureBusy ||
                  captureActive ||
                  captureSettling
                }
              >
                Start capture
              </button>
              <button
                className="mode-btn"
                onClick={() => void captureCommand('stop')}
                title="Stop the PX4 SD recording and wait for confirmation. This does not stop the motor test."
                disabled={!status.target_connected || captureBusy || !captureCanStop}
              >
                Stop capture
              </button>
            </div>
          </div>
          <div id="motor-logging-drawer" className="logging-drawer-body" hidden={!loggingDrawerOpen}>
          <div className="section-title logging-section">PX4 SD capture</div>

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
          {(captureError || (sdCapture.last_error && !captureErrorDismissed)) && (
            <InlineError
              message={captureError ?? captureErrorLabel(sdCapture.last_error)}
              onDismiss={() => {
                setCaptureError(null)
                setCaptureErrorDismissed(true)
              }}
            />
          )}
          <div className="btn-row logging-check-row" style={{ marginTop: 4 }}>
            <button
              className="mode-btn"
              onClick={() => void captureCommand('check')}
              title="Read the target logger status and logging parameters without starting or stopping motors."
              disabled={!status.target_connected || captureBusy || captureSettling}
            >
              Check logger
            </button>
          </div>
          <div className="estop-note" style={{ marginTop: 4 }}>
            SD capture is optional and controlled manually. START and STOP do
            not start or stop logging; capture can start or stop while the motor
            test runs. Delete logs only after capture has stopped.
          </div>

          <details className="logger-configuration">
            <summary>Logger configuration</summary>
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
          </details>

          <button
            className="section-toggle logging-section"
            aria-label="Log retrieval"
            aria-expanded={loggingExpanded}
            aria-controls="motor-log-retrieval"
            onClick={() => setLoggingExpanded((v) => !v)}
          >
            <span>Log retrieval</span>
            <span className="mono text-muted">
              {logsBusy ? 'Working…' : `${logEntries.length} files`}
              <span aria-hidden="true"> {loggingExpanded ? '▴' : '▾'}</span>
            </span>
          </button>
          {logsError && (
            <InlineError message={logsError} onDismiss={() => setLogsError(null)} />
          )}
          <div id="motor-log-retrieval" hidden={!loggingExpanded}>
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
        </div>
      </aside>
    </div>
  )
}
/** Persistent, dismissible inline failure for a panel-local operation. */
function InlineError({ message, onDismiss }: { message: string; onDismiss: () => void }) {
  return (
    <div className="inline-failure" role="alert">
      <span>{message}</span>
      <button className="link-btn" onClick={onDismiss} title="Dismiss this error">
        Dismiss
      </button>
    </div>
  )
}

/** The selected target's motor-test permission, never another FC's cache.
 * The main Parameters window is relevant only for a borrowed main target. */
function MotTestEnGate({ motTestEn, mainSerial }: { motTestEn: boolean | null; mainSerial: boolean }) {
  if (motTestEn !== false) return null
  const openParams = () => {
    void invoke('params_open').catch(() => {})
  }
  return (
    <div className="mot-en-gate">
      <span>
        COM_MOT_TEST_EN=0 — the selected PX4 rejects motor-test commands.
        Set this parameter to 1 on that flight controller before starting.
      </span>
      {mainSerial && <button className="mode-btn" style={{ width: '100%', marginTop: 6 }} onClick={openParams}>
        Open Parameters
      </button>}
    </div>
  )
}
