// Fixed motor-bench header: target identity, safety state and the two stop
// actions. It stays sticky at narrow widths, so Stop and Emergency Stop are
// always reachable while the workspace scrolls.

import type { ActuatorStatus } from './benchTypes'
import AppLogo from '../components/AppLogo'

export interface BenchFailure {
  operation: 'start' | 'stop' | 'emergency_stop'
  message: string
}

interface BenchHeaderProps {
  status: ActuatorStatus
  running: boolean
  starting: boolean
  releasing: boolean
  emergency: boolean
  unlocked: boolean
  stopReason: string | null
  failure: BenchFailure | null
  onStop: () => Promise<void>
  onEmergencyStop: () => void
  onResetEmergency: () => void
  onDismissFailure: () => void
}

function armedLabel(status: ActuatorStatus): { text: string; tone: string } {
  if (!status.target_connected) return { text: 'NO TARGET', tone: '' }
  if (!status.target_fc_alive) return { text: 'HEARTBEAT LOST', tone: 'text-error' }
  return status.armed
    ? { text: 'ARMED', tone: 'text-error' }
    : { text: 'DISARMED', tone: 'text-ok' }
}

function sdLabel(status: ActuatorStatus): { text: string; tone: string } {
  switch (status.sd_capture.phase) {
    case 'recording':
      return { text: 'RECORDING', tone: 'text-ok' }
    case 'existing_recording':
      return { text: 'EXISTING REC', tone: 'text-muted' }
    case 'failed':
      return { text: 'CAPTURE FAILED', tone: 'text-error' }
    default:
      return { text: 'SD —', tone: '' }
  }
}

export default function BenchHeader(props: BenchHeaderProps) {
  const { status } = props
  const armed = armedLabel(status)
  const sd = sdLabel(status)
  const target = status.target_connected
    ? `PX4 · ID ${status.target_system_id ?? '?'}`
    : 'No PX4 target'
  const stateText = props.emergency
    ? 'EMERGENCY'
    : props.releasing
      ? 'RELEASING'
      : props.running
        ? 'RUNNING'
        : props.starting
          ? 'STARTING'
          : props.unlocked
            ? 'UNLOCKED'
            : 'LOCKED'

  return (
    <header className="actuator-header">
      <span className="tool-header-brand">
        <AppLogo variant="wordmark" />
        <AppLogo variant="mark" />
      </span>
      <strong className="bench-title">Motor test</strong>

      <span className="header-chip" title={status.target_endpoint ?? 'No serial target'}>
        <span className="section-title">Target</span>
        <span className={`mono ${status.target_connected ? 'text-ok' : 'text-muted'}`}>
          {target}
        </span>
      </span>

      <span className="header-chip">
        <span className="section-title">FC</span>
        <span className={`mono ${armed.tone}`}>{armed.text}</span>
      </span>

      <span className="header-chip" title="Firmware release is not reported by this link">
        <span className="section-title">FW</span>
        <span className="mono text-muted">Unknown</span>
      </span>

      <span className="header-chip">
        <span className="section-title">SD</span>
        <span className={`mono ${sd.tone}`}>{sd.text}</span>
      </span>

      <span
        className="header-chip"
        title={`Measured command rate unavailable; configured ${status.tick_hz.toFixed(0)} Hz`}
      >
        <span className="section-title">Rate</span>
        <span className="mono">
          — <span className="text-muted">cfg {status.tick_hz.toFixed(0)} Hz</span>
        </span>
      </span>

      <span
        className={`bench-state ${props.emergency ? 'text-error' : props.running ? 'text-ok' : ''}`}
        data-testid="motor-session-state"
        aria-live="polite"
      >
        {stateText}
      </span>

      {props.failure ? (
        <span className="header-failure" role="alert">
          <span>{props.failure.operation.replaceAll('_', ' ')} failed: {props.failure.message}</span>
          <button
            className="link-btn"
            onClick={props.onDismissFailure}
            title="Dismiss this error"
          >
            Dismiss
          </button>
        </span>
      ) : props.stopReason ? (
        <span className="header-note">Stopped: {props.stopReason}</span>
      ) : (
        <span />
      )}

      <div className="header-actions">
        {props.emergency && (
          <button className="mode-btn header-btn" onClick={props.onResetEmergency}>
            RESET
          </button>
        )}
        <button
          className="mode-btn header-btn stop-tab"
          onClick={() => void props.onStop()}
          disabled={!props.running && !props.releasing}
          title="Release all configured motor outputs"
        >
          STOP
        </button>
        <button
          className={`estop-btn ${props.emergency ? 'latched' : ''}`}
          onClick={props.onEmergencyStop}
          title="Release all motors immediately (Esc)"
        >
          EMERGENCY STOP
        </button>
      </div>
    </header>
  )
}
