// Motor / mag interference test window — A1 skeleton (motor-test-plan §5–§6).
//
// Full-screen four-region frame: header (status + emergency stop), left
// control panel, middle state readout, right status panel. What is live in
// A1: the emergency stop (button + Esc, latched until an explicit reset),
// the load selector with the risk-scaled unlock, and real link/FC status.
// The sender itself is disabled until the M0 bench run pins the command
// semantics — a dead Start button would be a fake affordance (design rule 4).

import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { isTauri } from '../inspector/mock'

/** Prop load selected for this test. Operator-declared — there is no
 * sensor that can tell the types apart, and detection is explicitly out of
 * scope (operator decision 2026-10-10). */
type Load = 'none' | 'plate' | 'props'

const LOADS: Array<{ id: Load; label: string }> = [
  { id: 'none', label: 'No props' },
  { id: 'plate', label: 'Flat plate' },
  { id: 'props', label: 'Full props' },
]

/** One Hz status poll; the safety-critical signals (armed!) are low-rate. */
const STATUS_POLL_MS = 1000

interface ActuatorStatus {
  connected: boolean
  fc_alive: boolean
  armed: boolean
  mode: string
  endpoint: string | null
}

const IDLE: ActuatorStatus = {
  connected: false,
  fc_alive: false,
  armed: false,
  mode: '—',
  endpoint: null,
}

export function ActuatorApp() {
  const tauri = isTauri()
  const [status, setStatus] = useState<ActuatorStatus>(IDLE)
  const [load, setLoad] = useState<Load>('none')
  /** Unlocked = the operator removed the safety cover. Any load change
   * re-locks instantly (§8.2: the confirmation is bound to session+load). */
  const [unlocked, setUnlocked] = useState(false)
  /** Latched emergency: stays until an explicit reset, never auto-clears. */
  const [emergency, setEmergency] = useState(false)

  // Emergency stop: one gesture, always available. `Esc` is captured on the
  // window in the capture phase, so it works while focus is inside a slider
  // or text input; Space is deliberately NOT bound (plan 2.2-4).
  const estop = useCallback(() => setEmergency(true), [])

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

  // Live status from the shell (link, FC armed state). Browser dev shows the
  // inert defaults.
  useEffect(() => {
    if (!tauri) return
    const poll = async () => {
      try {
        setStatus(await invoke<ActuatorStatus>('actuator_status'))
      } catch {
        /* window opening before the link is up: keep the last status */
      }
    }
    void poll()
    const id = setInterval(poll, STATUS_POLL_MS)
    return () => clearInterval(id)
  }, [tauri])

  const changeLoad = (next: Load) => {
    setLoad(next)
    setUnlocked(false)
  }

  const resetEmergency = () => {
    setEmergency(false)
    setUnlocked(false)
  }

  return (
    <div className="actuator-grid">
      <header className="actuator-header">
        <strong style={{ fontSize: 13 }}>Motor / Mag Interference Test</strong>
        <span className={`row ${status.fc_alive ? '' : 'text-muted'}`} style={{ gap: 6 }}>
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

      {/* Left: control panel. Load → unlock → start, the fixed order. */}
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
              ? 'Full props: typed confirmation + 3 s hold before Start. Vehicle must be fixed to a rig.'
              : load === 'plate'
                ? 'Flat plates: same start path as no props.'
                : 'No props: safest configuration.'}
          </div>
        </div>

        <div className="card">
          <h3>Command source</h3>
          <div className="btn-row">
            <button className="mode-btn active">Manual</button>
            <button className="mode-btn" disabled title="Arrives with A6 (profile playback)">
              Profile
            </button>
            <button className="mode-btn" disabled title="Arrives with A7 (speed functions)">
              Function
            </button>
          </div>
        </div>

        <div className="card">
          <h3>Safety</h3>
          <button
            className="mode-btn"
            onClick={() => (emergency ? undefined : setUnlocked((u) => !u))}
            disabled={emergency}
            style={{ width: '100%' }}
          >
            {unlocked ? 'Re-lock test' : 'Unlock test'}
          </button>
          <button
            className="start-btn"
            style={{ marginTop: 8 }}
            disabled
            title="The sender is enabled after the M0 bench run pins the ACTUATOR_TEST semantics"
          >
            START
          </button>
          <div className="estop-note" style={{ marginTop: 6 }}>
            Sending is disabled until bench validation (M0) completes. Emergency stop
            works now.
          </div>
        </div>
      </aside>

      {/* Middle: state readout + interlock checklist. */}
      <main className="actuator-mid">
        <div
          className="card"
          style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            height: '100%',
            gap: 10,
            borderColor: emergency ? 'var(--mg-error)' : 'var(--mg-border)',
          }}
        >
          <div
            className="mono"
            style={{
              fontSize: 28,
              fontWeight: 700,
              letterSpacing: '0.12em',
              color: emergency ? 'var(--mg-error)' : 'var(--mg-muted)',
            }}
          >
            {emergency ? 'EMERGENCY' : unlocked ? 'READY' : 'LOCKED'}
          </div>
          <div className="estop-note">Motor commands: stopped · sender: disabled (M0 pending)</div>
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
        </div>
        <div className="estop-note">
          Tests run only while the FC is disarmed. Esc = emergency stop from anywhere in
          this window.
        </div>
      </aside>
    </div>
  )
}