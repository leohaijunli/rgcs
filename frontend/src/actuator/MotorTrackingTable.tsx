// Motor comparison: source value, expected physical output (inverted from the
// ACTUATOR_TEST value actually sent), target-reported output and their error.
// Missing measurements stay "—"; no value is inferred from a stale sample.
//
// PWM/OneShot numbers are microseconds; DShot/BDShot numbers are PX4 mixer
// units (0…1999, the same scale as SERVO_OUTPUT_RAW for those protocols).

import { Fragment, useState } from 'react'
import {
  activeMotorCount,
  assignmentFor,
  channelLabel,
  isDShot,
  outputUnits,
  outputUnitLabel,
  outputConfigurationIssue,
  protocolLabel,
  type ActuatorStatus,
  type PwmModel,
} from './benchTypes'

/** Requested source of the motor demand. */
type BenchSource = 'manual' | 'preset' | 'forward'

/** Tracking tolerance: ±5 µs for pulse outputs, ±2 mixer counts for DShot. */
const TRACKING_TOLERANCE_US = 5
const TRACKING_TOLERANCE_DSHOT = 2

interface MotorTrackingTableProps {
  status: ActuatorStatus
  statusFresh: boolean
  source: BenchSource
  values: number[]
  running: boolean
}

function sourceValue(
  status: ActuatorStatus,
  source: BenchSource,
  values: number[],
  motor: number,
): string {
  if (source === 'forward') {
    const pulse = status.input_pwm[motor]
    return pulse != null ? `${pulse} µs` : '—'
  }
  if (source === 'preset') return 'waveform'
  const value = values[motor]
  return value != null ? `${(value * 100).toFixed(0)}%` : '—'
}

function toleranceFor(model: PwmModel | undefined): number {
  return isDShot(model?.protocol) ? TRACKING_TOLERANCE_DSHOT : TRACKING_TOLERANCE_US
}

function formatOutput(value: number | null, model: PwmModel | undefined): string {
  if (value == null) return '—'
  return `${value.toFixed(0)} ${outputUnits(model)}`
}

function formatDelta(delta: number | null, model: PwmModel | undefined): string {
  if (delta == null) return '—'
  return `${delta > 0 ? '+' : ''}${delta.toFixed(0)} ${outputUnits(model)}`
}

function stateFor(
  status: ActuatorStatus,
  fresh: boolean,
  running: boolean,
  model: PwmModel | undefined,
  mapped: boolean,
  reported: number | null,
  expected: number | null,
  delta: number | null,
): { text: string; tone: string } {
  if (!status.target_connected) return { text: 'no target', tone: 'text-muted' }
  if (!fresh) return { text: 'stale link', tone: 'text-error' }
  if (!mapped) return { text: 'unmapped', tone: 'text-warn' }
  if (!model) return { text: 'no model', tone: 'text-muted' }
  if (model.protocol.kind === 'unknown') return { text: 'protocol unknown', tone: 'text-warn' }
  if (model.range_source !== 'fc') return { text: 'range unread', tone: 'text-warn' }
  if (!running) return { text: 'idle', tone: 'text-muted' }
  if (expected == null) return { text: 'released', tone: 'text-muted' }
  if (reported == null) return { text: 'no feedback', tone: 'text-warn' }
  if (delta != null && Math.abs(delta) <= toleranceFor(model)) {
    return { text: 'tracking', tone: 'text-ok' }
  }
  return { text: 'mismatch', tone: 'text-error' }
}

export default function MotorTrackingTable(props: MotorTrackingTableProps) {
  const { status, statusFresh, source, values, running } = props
  const [expanded, setExpanded] = useState<Set<number>>(() => new Set())
  const [open, setOpen] = useState(false)
  const motors = activeMotorCount(status.output_map)
  const activeModels = status.pwm_models.slice(0, motors)
  const gap = status.output_map.motors.some(
    (assignment) => assignment.motor > motors && assignment.channel != null,
  )
  const feedbackStale = status.target_connected && !statusFresh
  const configurationWarning = status.target_connected && outputConfigurationIssue(status) !== null
  const requiredOpen = running || feedbackStale || configurationWarning || gap
  const visible = requiredOpen || open

  const toggle = (motor: number) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(motor)) next.delete(motor)
      else next.add(motor)
      return next
    })
  }

  return (
    <div className="card motor-tracking">
      <div className="tracking-head">
        <div className="tracking-title">
          <h3>Motor tracking</h3>
          <span className={`chip ${motors > 0 ? 'chip-ok' : 'chip-warn'}`}>
            {motors > 0 ? `${motors} motor${motors === 1 ? '' : 's'}` : 'no motors'}
          </span>
        </div>
        <div className="tracking-actions">
          <span className="estop-note tracking-hint">
            Command is converted by the sender using the PX4 output range, after
            safety limits and compensation.
          </span>
          <button
            className="link-btn"
            aria-expanded={visible}
            aria-controls="motor-tracking-body"
            data-testid="motor-tracking-fold"
            disabled={requiredOpen}
            onClick={() => setOpen((value) => !value)}
          >
            {requiredOpen ? 'required' : visible ? 'Fold' : 'Unfold'}
          </button>
        </div>
      </div>
      <div id="motor-tracking-body" hidden={!visible}>
        <div className="motor-table-wrap">
          <table className="motor-table">
            <thead>
              <tr>
                <th>Motor</th>
                <th>Output</th>
                <th>PX4 protocol</th>
                <th>Range</th>
                <th>Source</th>
                <th>Command</th>
                <th>Reported</th>
                <th>Δ</th>
                <th>State</th>
                <th aria-label="Details" />
              </tr>
            </thead>
            <tbody>
              {motors === 0 ? (
                <tr>
                  <td colSpan={10} className="empty-row">
                    No configured motor outputs — assign a motor function and
                    refresh the mapping.
                  </td>
                </tr>
              ) : (
                Array.from({ length: motors }, (_, index) => {
                  const motor = index + 1
                  const assignment = assignmentFor(status.output_map, motor)
                  const channel = assignment?.channel ?? null
                  const model = status.pwm_models[index]
                  const unit = outputUnits(model)
                  const command = status.output_values[index]
                  const expected =
                    running && channel ? status.output_pwm?.[index] ?? null : null
                  const rawReported = status.fc_output_pwm[index]
                  const reported = statusFresh && rawReported != null ? rawReported : null
                  const delta =
                    unit !== 'raw' && expected != null && reported != null ? reported - expected : null
                  const state = stateFor(
                    status,
                    statusFresh,
                    running,
                    model,
                    channel != null,
                    reported,
                    expected,
                    delta,
                  )
                  const open = expanded.has(motor)
                  return (
                    <Fragment key={motor}>
                      <tr>
                        <td className="mono">M{motor}</td>
                        <td className="mono">
                          {channel ? channelLabel(channel) : 'unmapped'}
                        </td>
                        <td className="mono">
                          <span className={`protocol-chip ${isDShot(model?.protocol) ? 'protocol-dshot' : ''}`}>
                            {protocolLabel(model?.protocol)}
                          </span>
                        </td>
                        <td
                          className={`mono ${model?.range_source === 'fc' ? '' : 'text-warn'}`}
                          data-testid={`motor-range-${motor}`}
                        >
                          {channel && model?.range_source === 'fc'
                            ? unit === 'raw'
                              ? 'Unit unknown'
                              : `${model.min_us}–${model.max_us} ${unit}`
                            : 'Not read'}
                        </td>
                        <td className="mono">{sourceValue(status, source, values, index)}</td>
                        <td
                          className="mono"
                          data-testid={`motor-command-pwm-${motor}`}
                          title="Converted from the sender's queued ACTUATOR_TEST command; not measured feedback"
                        >
                          {formatOutput(expected, model)}
                        </td>
                        <td className="mono">{formatOutput(reported, model)}</td>
                        <td className="mono">{formatDelta(delta, model)}</td>
                        <td className={`mono ${state.tone}`}>{state.text}</td>
                        <td>
                          <button
                            className="link-btn"
                            onClick={() => toggle(motor)}
                            aria-expanded={open}
                            aria-label={`Motor ${motor} details`}
                          >
                            {open ? 'Hide' : 'Details'}
                          </button>
                        </td>
                      </tr>
                      {open && (
                        <tr className="motor-detail" key={`${motor}-detail`}>
                          <td colSpan={10}>
                            <span>
                              Function Motor{motor} →{' '}
                              {channel ? channelLabel(channel) : 'no mapped output'}
                            </span>
                            <span>
                              Protocol {protocolLabel(model?.protocol)}
                            </span>
                            <span>
                              Range{' '}
                              {model
                                ? unit === 'raw'
                                  ? 'unit unknown'
                                  : `${model.min_us}–${model.max_us} ${outputUnitLabel(model)}`
                                : 'unknown'}{' '}
                              {model ? (
                                <em
                                  className={
                                    model.range_source === 'fallback'
                                      ? 'text-error'
                                      : undefined
                                  }
                                >
                                  {model.range_source === 'fc'
                                    ? '(from PX4)'
                                    : '(fallback, not read)'}
                                </em>
                              ) : null}
                            </span>
                            <span>
                              Reversed {model ? (model.reversed ? 'yes' : 'no') : '—'}
                            </span>
                            <span>
                              THR_MDL_FAC{' '}
                              {model ? model.thrust_factor.toFixed(2) : '—'}
                            </span>
                            {isDShot(model?.protocol) ? (
                              <span>
                                DSHOT_MIN{' '}
                                {model ? model.dshot_min.toFixed(3) : '—'}
                              </span>
                            ) : null}
                            <span>
                              ACTUATOR_TEST (0–1){' '}
                              {running && command != null ? command.toFixed(6) : '—'}
                            </span>
                          </td>
                        </tr>
                      )}
                    </Fragment>
                  )
                })
              )}
            </tbody>
          </table>
        </div>
        {activeModels.some((model) => model.range_source === 'fallback') ? (
          <div className="estop-note text-error">
            Range not read from the PX4 for{' '}
            {activeModels
              .map((model, index) =>
                model.range_source === 'fallback' ? `M${index + 1}` : null,
              )
              .filter((motor) => motor != null)
              .join(', ')}{' '}
            — starting is blocked. Refresh after checking the serial link and
            the matching PX4 parameters.
          </div>
        ) : null}
        {gap ? (
          <div className="estop-note text-error">
            Mapping has a gap after M{motors} — a motor function is assigned
            beyond the contiguous M1..MN prefix. Fix PWM_MAIN/AUX_FUNCx and
            refresh.
          </div>
        ) : null}
        {status.output_map.state === 'invalid' || status.output_map.issues.length > 0 ? (
          <div className="estop-note text-error">
            Mapping has {status.output_map.issues.length} issue
            {status.output_map.issues.length === 1 ? '' : 's'} — refresh after
            checking PWM_MAIN/AUX_FUNCx.
          </div>
        ) : null}
      </div>
    </div>
  )
}
