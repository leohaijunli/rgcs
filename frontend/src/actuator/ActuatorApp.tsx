// Motor / mag interference test window — A1+A3 (motor-test-plan §5–§6, §8.2),
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
import { isTauri } from '../inspector/mock'

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
    hint: 'Realtime SITL→FC forwarding (pending design)',
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
  mode: string
  endpoint: string | null
  tick_hz: number
  session: string
}

const IDLE: ActuatorStatus = {
  connected: false,
  fc_alive: false,
  armed: false,
  mode: '—',
  endpoint: null,
  tick_hz: 10,
  session: 'idle',
}

export function ActuatorApp() {
  const tauri = isTauri()
  const [status, setStatus] = useState<ActuatorStatus>(IDLE)
  const [load, setLoad] = useState<Load>('none')
  const [source, setSource] = useState<Source>('manual')
  /** Unlocked = the operator removed the safety cover. Any load change
   * re-locks instantly (§8.2: the confirmation binds to session+load). */
  const [unlocked, setUnlocked] = useState(false)
  /** Latched emergency: stays until an explicit reset, never auto-clears. */
  const [emergency, setEmergency] = useState(false)
  // Manual source state.
  const [values, setValues] = useState<number[]>(() => new Array(MOTOR_COUNT).fill(0))
  const [sync, setSync] = useState(true)
  // Preset source state.
  const [preset, setPreset] = useState<PresetKind>('ramp')
  const [amplitude, setAmplitude] = useState(0.5)
  const [frequency, setFrequency] = useState(0.5)
  const [duration, setDuration] = useState(5)
  // Confirmation state.
  const [phrase, setPhrase] = useState('')
  const [holdProgress, setHoldProgress] = useState(0)

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
  useEffect(() => {
    if (!tauri) return
    const poll = async () => {
      try {
        const s = await invoke<ActuatorStatus>('actuator_status')
        setStatus(s)
        if (s.session === 'emergency') setEmergency(true)
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
        un = await listen<unknown>('actuator_event', () => {
          if (disposed) return
          void invoke<ActuatorStatus>('actuator_status')
            .then(setStatus)
            .catch(() => {})
        })
      } catch {
        /* events unavailable: the poll still runs */
      }
    })()
    return () => {
      disposed = true
      un?.()
    }
  }, [tauri])

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
    if (source === 'manual') {
      void invoke('actuator_start_manual', { load, values: valuesRef.current }).catch(() => {})
    } else if (source === 'preset') {
      void invoke('actuator_start_preset', {
        load,
        preset: {
          kind: preset,
          amplitude,
          frequency_hz: frequency,
          duration_s: duration,
        },
      }).catch(() => {})
    }
    // 'forward' is gated off until the forwarding work package lands.
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
    <div className="actuator-grid">
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

      {/* Left: one workflow — load → source → source panel → safety. */}
      <aside className="actuator-left">
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
            <div className="estop-note">
              Forwards live SITL actuator commands to the FC. Pending design:
              which SITL signal feeds it and whether the GCS carries two links
              at once.
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
              disabled={gate !== 'ready' || source === 'forward'}
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
                        ? 'Forwarding arrives with its work package'
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
      </aside>

      {/* Middle: compact state strip + reserved monitoring space. */}
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
              : 'Motor commands stopped · Esc = emergency stop'}
          </span>
        </div>
        <div className="reserved-area estop-note">
          Realtime curves (A5) and the camera preview land here.
        </div>
      </main>

      {/* Right: live status. */}
      <aside className="actuator-right">
        <div className="section-title">Status</div>
        <div className="card">
          <div className="row">
            <span className="text-muted">Link</span>
            <span className={`mono ${status.connected ? 'text-ok' : 'text-error'}`}>
              {status.connected ? 'connected' : 'down'}
            </span>
          </div>
          <div className="row">
            <span className="text-muted">FC heartbeat</span>
            <span className="mono">{status.fc_alive ? 'alive' : 'lost'}</span>
          </div>
          <div className="row">
            <span className="text-muted">Endpoint</span>
            <span className="mono" title={status.endpoint ?? ''}>
              {status.endpoint ?? '—'}
            </span>
          </div>
        </div>
        <div className="card">
          <div className="row">
            <span className="text-muted">FC mode</span>
            <span className="mono">{status.mode}</span>
          </div>
          <div className="row">
            <span className="text-muted">Armed</span>
            <span className="mono">{status.armed ? 'YES — sending forbidden' : 'no'}</span>
          </div>
          <div className="row">
            <span className="text-muted">Session</span>
            <span className="mono">{status.session}</span>
          </div>
        </div>
        <div className="estop-note">
          Tests run while the FC is disarmed; PX4 denies ACTUATOR_TEST otherwise
          (enable COM_MOT_TEST_EN=1 on the FC).
        </div>
      </aside>
    </div>
  )
}