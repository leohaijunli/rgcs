// Motor comparison: source value, expected physical pulse (inverted from the
// ACTUATOR_TEST value actually sent), target-reported pulse and their error.
// Missing measurements stay "—"; no value is inferred from a stale sample.

import { Fragment, useState } from 'react'
import {
  MOTOR_COUNT,
  assignmentFor,
  channelLabel,
  type ActuatorStatus,
  type PwmModel,
} from './benchTypes'

/** Requested source of the motor demand. */
type BenchSource = 'manual' | 'preset' | 'forward'

/** Tolerance for calling a mapped output as tracking (bench criterion ±5 µs). */
const TRACKING_TOLERANCE_US = 5

interface MotorTrackingTableProps {
  status: ActuatorStatus
  statusFresh: boolean
  source: BenchSource
  values: number[]
  running: boolean
}

/** Invert the uncompensated ACTUATOR_TEST value back to a physical pulse.
 * Mirrors `PwmModel::pulse_for_command` in core; null when the model or the
 * command cannot produce a physical output. */
function expectedPulse(model: PwmModel | undefined, command: number | undefined): number | null {
  if (!model || command == null) return null
  const span = model.max_us - model.min_us
  if (!(model.min_us < model.max_us) || span <= 0) return null
  if (!Number.isFinite(command) || command < 0 || command > 1) return null
  const a = model.thrust_factor
  if (!Number.isFinite(a) || a < 0 || a > 1) return null
  let x: number
  if (a === 0) {
    x = command
  } else {
    const linear = 1 - a
    const discriminant = linear * linear + 4 * a * command
    if (discriminant < 0) return null
    x = (-linear + Math.sqrt(discriminant)) / (2 * a)
  }
  const fraction = model.reversed ? 1 - x : x
  const pulse = model.min_us + fraction * span
  return Number.isFinite(pulse) ? pulse : null
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

function stateFor(
  status: ActuatorStatus,
  fresh: boolean,
  running: boolean,
  model: PwmModel | undefined,
  mapped: boolean,
  reported: number | null,
  delta: number | null,
): { text: string; tone: string } {
  if (!status.target_connected) return { text: 'no target', tone: 'text-muted' }
  if (!fresh) return { text: 'stale link', tone: 'text-error' }
  if (!mapped) return { text: 'unmapped', tone: 'text-warn' }
  if (!model) return { text: 'no model', tone: 'text-muted' }
  if (model.disabled) return { text: 'disabled', tone: 'text-warn' }
  if (!running) return { text: 'idle', tone: 'text-muted' }
  if (reported == null) return { text: 'no feedback', tone: 'text-warn' }
  if (delta != null && Math.abs(delta) <= TRACKING_TOLERANCE_US) {
    return { text: 'tracking', tone: 'text-ok' }
  }
  return { text: 'mismatch', tone: 'text-error' }
}

export default function MotorTrackingTable(props: MotorTrackingTableProps) {
  const { status, statusFresh, source, values, running } = props
  const [expanded, setExpanded] = useState<Set<number>>(() => new Set())

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
        <h3>Motor tracking</h3>
        <span className="estop-note">
          Expected is inverted from the sent command; safety limits may reduce
          demand before encoding.
        </span>
      </div>
      <div className="motor-table-wrap">
        <table className="motor-table">
          <thead>
            <tr>
              <th>Motor</th>
              <th>Output</th>
              <th>Source</th>
              <th>Expected</th>
              <th>Reported</th>
              <th>Δ</th>
              <th>State</th>
              <th aria-label="Details" />
            </tr>
          </thead>
          <tbody>
            {Array.from({ length: MOTOR_COUNT }, (_, index) => {
              const motor = index + 1
              const assignment = assignmentFor(status.output_map, motor)
              const channel = assignment?.channel ?? null
              const model = status.pwm_models[index]
              const command = status.output_values[index]
              const expected =
                running && channel && model ? expectedPulse(model, command) : null
              const rawReported = status.fc_output_pwm[index]
              const reported = statusFresh && rawReported != null ? rawReported : null
              const delta =
                expected != null && reported != null ? reported - expected : null
              const state = stateFor(
                status,
                statusFresh,
                running,
                model,
                channel != null,
                reported,
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
                    <td className="mono">{sourceValue(status, source, values, index)}</td>
                    <td className="mono" title="Physical pulse expected from the last sent command">
                      {expected != null ? `${expected.toFixed(0)} µs` : '—'}
                    </td>
                    <td className="mono">
                      {reported != null ? `${reported} µs` : '—'}
                    </td>
                    <td className="mono">
                      {delta != null
                        ? `${delta > 0 ? '+' : ''}${delta.toFixed(0)} µs`
                        : '—'}
                    </td>
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
                      <td colSpan={8}>
                        <span>
                          Function Motor{motor} →{' '}
                          {channel ? channelLabel(channel) : 'no mapped output'}
                        </span>
                        <span>
                          Range{' '}
                          {model
                            ? `${model.min_us}–${model.max_us} µs`
                            : 'unknown'}
                        </span>
                        <span>
                          Reversed {model ? (model.reversed ? 'yes' : 'no') : '—'}
                        </span>
                        <span>
                          THR_MDL_FAC{' '}
                          {model ? model.thrust_factor.toFixed(2) : '—'}
                        </span>
                        <span>
                          ACTUATOR_TEST{' '}
                          {running && command != null ? command.toFixed(6) : '—'}
                        </span>
                      </td>
                    </tr>
                  )}
                </Fragment>
              )
            })}
          </tbody>
        </table>
      </div>
      {status.output_map.state === 'invalid' || status.output_map.issues.length > 0 ? (
        <div className="estop-note text-error">
          Mapping has {status.output_map.issues.length} issue
          {status.output_map.issues.length === 1 ? '' : 's'} — refresh after
          checking PWM_MAIN/AUX_FUNCx.
        </div>
      ) : null}
    </div>
  )
}
