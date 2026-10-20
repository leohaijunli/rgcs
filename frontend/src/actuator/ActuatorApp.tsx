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
import { isTauri } from '../inspector/mock'
import SignalScope, { useScopeFeed } from './SignalScope'
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
  sitl_feed_age_s: number | null
  /** The motor test's own target link (real PX4, serial). */
  target_connected: boolean
  target_endpoint: string | null
  target_system_id: number | null
  /** COM_MOT_TEST_EN read from the target PX4 (null = unknown). */
  com_mot_test_en: number | null
  /** Normalized SITL servo outputs 1–4 (the forward INPUT). */
  input_values: number[]
  /** Values sent to the FC as ACTUATOR_TEST (the OUTPUT), 0 when idle. */
  output_values: number[]
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
  sitl_feed_age_s: null,
  target_connected: false,
  target_endpoint: null,
  target_system_id: null,
  com_mot_test_en: null,
  input_values: [],
  output_values: [],
}

/** Middle column content: the control cards, or the signal scope. */
type MidView = 'controls' | 'scope'

export function ActuatorApp() {
  const tauri = isTauri()
  // Collect scope samples for the window's whole lifetime, so the history
  // is there when the operator switches to the Scope tab.
  useScopeFeed()
  const [view, setView] = useState<MidView>('controls')
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
  // Realtime forward (revised D3): the SITL source endpoint.
  const [sitlEndpoint, setSitlEndpoint] = useState('udpin:0.0.0.0:14540')
  // The motor test's own target link: the real PX4 over serial (the main
  // GCS keeps its own link, e.g. UDP SITL).
  const [targetEndpoint, setTargetEndpoint] = useState('serial:/dev/ttyACM0:115200')
  const [targetSystemId, setTargetSystemId] = useState(1)
  const [targetConnectError, setTargetConnectError] = useState<string | null>(null)
  // Why the last session stopped (armed / heartbeat_stale / acks_denied / …).
  const [stopReason, setStopReason] = useState<string | null>(null)
  // Confirmation state.
  const [phrase, setPhrase] = useState('')
  const [holdProgress, setHoldProgress] = useState(0)
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
  const audioCtxRef = useRef<AudioContext | null>(null)
  const beepTimer = useRef<number | undefined>(undefined)

  const valuesRef = useRef(values)
  valuesRef.current = values
  const lastSend = useRef(0)
  const holdTimer = useRef<number | undefined>(undefined)
  const holdStart = useRef(0)

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
          setStartError(null)
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

  const stop = () => {
    if (tauri) void invoke('actuator_stop').catch(() => {})
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
  const start = () => {
    if (!tauri || gate !== 'ready' || running) return
    setStartError(null)
    // The backend refuses (e.g. "no link — connect the serial PX4 first");
    // surface it instead of silently doing nothing.
    const refuse = (e: unknown) => setStartError(typeof e === 'string' ? e : String(e))
    if (source === 'manual') {
      void invoke('actuator_start_manual', { load, values: valuesRef.current }).catch(refuse)
    } else if (source === 'preset') {
      void invoke('actuator_start_preset', {
        load,
        preset: {
          kind: preset,
          amplitude,
          frequency_hz: frequency,
          duration_s: duration,
        },
      }).catch(refuse)
    } else if (source === 'forward') {
      void invoke('actuator_start_forward').catch(refuse)
    }
  }

  // Hold-to-start (full props): pointerdown begins the ramp; release before
  // the full duration cancels; completion fires start once.
  const holdPct = load === 'props' ? holdProgress : 1
  const beginHold = () => {
    if (gate !== 'ready' || running) return
    if (load !== 'props') {
      start()
      return
    }
    holdStart.current = performance.now()
    const step = () => {
      const elapsed = performance.now() - holdStart.current
      const pct = Math.min(1, elapsed / PROPS_HOLD_MS)
      setHoldProgress(pct)
      if (pct >= 1) {
        holdTimer.current = undefined
        start()
        setHoldProgress(0)
        return
      }
      holdTimer.current = window.requestAnimationFrame(step)
    }
    holdTimer.current = window.requestAnimationFrame(step)
  }
  const cancelHold = () => {
    if (holdTimer.current !== undefined) {
      window.cancelAnimationFrame(holdTimer.current)
      holdTimer.current = undefined
    }
    setHoldProgress(0)
  }
  useEffect(() => cancelHold, [])

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
          <input
            className="phrase-input"
            style={{ width: '100%', margin: 0 }}
            value={sitlEndpoint}
            onChange={(e) => setSitlEndpoint(e.target.value)}
            placeholder="udpin:0.0.0.0:14540"
            aria-label="SITL endpoint"
            spellCheck={false}
          />
          {status.sitl_connected ? (
            <button
              className="mode-btn"
              style={{ width: '100%', marginTop: 6 }}
              onClick={() => tauri && void invoke('actuator_disconnect_sitl').catch(() => {})}
            >
              Disconnect SITL
            </button>
          ) : (
            <button
              className="mode-btn"
              style={{ width: '100%', marginTop: 6 }}
              onClick={() => tauri && void invoke('actuator_connect_sitl', { endpoint: sitlEndpoint }).catch(() => {})}
            >
              Connect SITL
            </button>
          )}
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
              return (
                <div key={i} className="motor-bar">
                  <span className="mono motor-label">M{i + 1}</span>
                  <div className="bar-track">
                    <div className="bar-fill input" style={{ width: `${v * 100}%` }} />
                  </div>
                  <span className="mono motor-value">{(v * 100).toFixed(0)}%</span>
                </div>
              )
            })}
          </div>
          <div className="estop-note" style={{ marginTop: 4 }}>
            SITL main outputs 1–4, normalized 0..1 → the forward slot.
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
            <button
              className={`mode-btn view-tab ${view === 'controls' ? 'active' : ''}`}
              onClick={() => setView('controls')}
            >
              Controls
            </button>
            <button
              className={`mode-btn view-tab ${view === 'scope' ? 'active' : ''}`}
              onClick={() => setView('scope')}
            >
              Scope
            </button>
          </span>
        </div>
        {view === 'scope' && (
          <SignalScope
            sitlConnected={status.sitl_connected}
            targetConnected={status.target_connected}
            calLoaded={calLoaded}
          />
        )}
        <div className="controls-grid" style={view === 'scope' ? { display: 'none' } : undefined}>
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
            <input
              className="phrase-input"
              style={{ width: '100%', margin: 0 }}
              value={sitlEndpoint}
              onChange={(e) => setSitlEndpoint(e.target.value)}
              placeholder="udpin:0.0.0.0:14540"
              aria-label="SITL endpoint"
              spellCheck={false}
            />
            {status.sitl_connected ? (
              <button
                className="mode-btn"
                style={{ width: '100%', marginTop: 6 }}
                onClick={() => tauri && void invoke('actuator_disconnect_sitl').catch(() => {})}
              >
                Disconnect SITL
              </button>
            ) : (
              <button
                className="mode-btn"
                style={{ width: '100%', marginTop: 6 }}
                onClick={() => tauri && void invoke('actuator_connect_sitl', { endpoint: sitlEndpoint }).catch(() => {})}
              >
                Connect SITL
              </button>
            )}
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
              SITL main outputs 1–4 → motors 1–4. Values pass the same
              interlocks (slew, cap, disarmed-only) as manual control.
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
          {running ? (
            <button className="start-btn stop" style={{ marginTop: 8 }} onClick={stop}>
              STOP
            </button>
          ) : (
            <button
              className="start-btn"
              style={{ marginTop: 8, position: 'relative' }}
              disabled={
                gate !== 'ready' || (source === 'forward' && !status.sitl_connected)
              }
              onPointerDown={beginHold}
              onPointerUp={cancelHold}
              onPointerLeave={cancelHold}
              title={
                gate === 'emergency'
                  ? 'Reset the emergency latch first'
                  : gate === 'unlock'
                    ? 'Unlock the test first'
                    : gate === 'phrase'
                      ? `Type ${PROPS_PHRASE} first`
                      : source === 'forward'
                        ? 'Connect the SITL source first'
                        : load === 'props'
                          ? 'Hold for 3 seconds'
                          : 'Start sending'
              }
            >
              START
              {holdPct > 0 && holdPct < 1 && (
                <span className="hold-fill" style={{ width: `${holdPct * 100}%` }} aria-hidden />
              )}
            </button>
          )}
          <div className="estop-note" style={{ marginTop: 6 }}>
            {running
              ? 'Sending — Esc or the red button stops everything.'
              : 'No arming required: the test runs while the FC is disarmed.'}
          </div>
        </div>
      </div>
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
          <div className="motor-bars" style={{ marginTop: 6 }}>
            {Array.from({ length: MOTOR_COUNT }, (_, i) => {
              const v = status.output_values[i] ?? 0
              return (
                <div key={i} className="motor-bar">
                  <span className="mono motor-label">M{i + 1}</span>
                  <div className="bar-track">
                    <div className="bar-fill output" style={{ width: `${v * 100}%` }} />
                  </div>
                  <span className="mono motor-value">{(v * 100).toFixed(0)}%</span>
                </div>
              )
            })}
          </div>
          <div className="estop-note" style={{ marginTop: 4 }}>
            Every ACTUATOR_TEST goes out over this serial link to the real PX4
            (the main GCS keeps its own link — e.g. UDP SITL).
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

        <MotTestEnGate motTestEn={motTestEn} />

        <div className="estop-note">
          Tests run while the FC is disarmed; PX4 denies ACTUATOR_TEST otherwise
          (enable COM_MOT_TEST_EN=1 on the FC).
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
        COM_MOT_TEST_EN=0 — the FC rejects every motor-test command. Set it to
        1 in Parameters (reboot required after Apply).
      </span>
      <button className="mode-btn" style={{ width: '100%', marginTop: 6 }} onClick={openParams}>
        Open Parameters
      </button>
    </div>
  )
}
