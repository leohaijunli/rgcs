// Fixed motor-bench header: target identity, safety state and the two stop
// actions. It stays sticky at narrow widths, so Stop and Emergency Stop are
// always reachable while the workspace scrolls.

import {
  activeProtocolSummary,
  outputConfigurationIssue,
  type ActuatorStatus,
} from './benchTypes'
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

export default function BenchHeader(props: BenchHeaderProps) {
  const { status } = props
  const armed = armedLabel(status)
  const protocol = activeProtocolSummary(status)
  const outputIssue = outputConfigurationIssue(status)
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

      <span
        className={`header-chip ${outputIssue ? 'text-warn' : ''}`}
        title={outputIssue ?? `Active PX4 output protocol: ${protocol}`}
        data-testid="motor-active-protocol"
      >
        <span className="section-title">Output</span>
        <span className="mono">{protocol}</span>
        {outputIssue && <span className="header-blocked">BLOCKED</span>}
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
      ) : outputIssue ? (
        <span className="header-note text-warn" data-testid="motor-output-warning">
          {outputIssue}
        </span>
      ) : (
        <span />
      )}

      <div className="header-actions">
        {props.emergency && (
          <button className="mode-btn header-btn" onClick={props.onResetEmergency} title="Clear the Motor Test emergency latch after release is confirmed. This never starts motors.">
            RESET
          </button>
        )}
        <button
          className="mode-btn header-btn stop-tab"
          onClick={() => void props.onStop()}
          disabled={!props.running && !props.releasing && !props.starting}
          title={props.starting ? 'Cancel the pending Motor Test start.' : props.running || props.releasing ? 'Release outputs owned by Motor Test. Manual recording continues; the flight controller is not disarmed.' : 'No Motor Test output is currently owned. STOP does not take control from another controller.'}
        >
          STOP
        </button>
        <button
          className={`estop-btn ${props.emergency ? 'latched' : ''}`}
          onClick={props.onEmergencyStop}
          title="Latch Motor Test emergency stop (Esc). Release only this tool’s owned outputs; do not arm or disarm the flight controller."
        >
          EMERGENCY STOP
        </button>
      </div>
    </header>
  )
}
