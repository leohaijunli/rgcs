// Shared motor-bench status contracts. These mirror the Rust payloads from
// `ActuatorStatus`; keep them beside the window so the tracking table and
// header do not reach into the panel component.

import type { ScopeOutputMeta } from './scopeStore'

export type MotorTone = 'off' | 'rising' | 'falling' | 'sweep' | 'triple'
export interface MotorToneSettings { start: MotorTone; stop: MotorTone }
export interface MotorToneStatus {
  settings: MotorToneSettings
  starting: boolean
  busy: boolean
  phase: 'idle' | 'queued' | 'checking' | 'playing' | 'complete' | 'failed' | 'cancelled' | 'unavailable'
  trigger: 'starting' | 'stopped' | null
  error: string | null
}
export const DEFAULT_MOTOR_TONES: MotorToneSettings = { start: 'rising', stop: 'falling' }
export const IDLE_TONES: MotorToneStatus = {
  settings: DEFAULT_MOTOR_TONES, starting: false, busy: false,
  phase: 'idle', trigger: null, error: null,
}

/** Hardware maximum. The bench renders only the configured M1..MN prefix. */
export const MAX_MOTOR_COUNT = 4
/** @deprecated Prefer `activeMotorCount(status.output_map)`; kept for callers
 * that still need the array width. */
export const MOTOR_COUNT = MAX_MOTOR_COUNT

export type DShotRate = 'r150' | 'r300' | 'r600'

export type OutputProtocol =
  | { kind: 'pwm'; rate_hz: number | null }
  | { kind: 'oneshot' }
  | { kind: 'dshot'; rate: DShotRate }
  | { kind: 'bdshot'; rate: DShotRate }
  | { kind: 'unknown' }

export interface OutputProtocols {
  main: OutputProtocol
  aux: OutputProtocol
}

export function isDShot(protocol: OutputProtocol | undefined): boolean {
  return protocol?.kind === 'dshot' || protocol?.kind === 'bdshot'
}

export function protocolLabel(protocol: OutputProtocol | undefined): string {
  if (!protocol) return 'unknown'
  switch (protocol.kind) {
    case 'pwm':
      return protocol.rate_hz != null ? `PWM ${protocol.rate_hz} Hz` : 'PWM'
    case 'oneshot':
      return 'OneShot'
    case 'dshot':
      return `DShot${protocol.rate.slice(1)}`
    case 'bdshot':
      return `BDShot${protocol.rate.slice(1)}`
    case 'unknown':
      return 'unknown'
  }
}

/** Number of configured motors in the contiguous M1..MN prefix. */
export function activeMotorCount(map: OutputMap | undefined): number {
  if (!map) return 0
  let count = 0
  for (const assignment of map.motors) {
    if (assignment.motor !== count + 1 || assignment.channel == null) break
    count = assignment.motor
  }
  return count
}

/** Active motors plus a mapping gap after the prefix. */
export function mappingHasGap(map: OutputMap | undefined): boolean {
  if (!map) return false
  const active = activeMotorCount(map)
  return map.motors.some(
    (assignment) => assignment.motor > active && assignment.channel != null,
  )
}

export function outputUnits(model: PwmModel | undefined): 'µs' | 'thr' | 'raw' {
  if (!model || model.protocol.kind === 'unknown') return 'raw'
  return isDShot(model?.protocol) ? 'thr' : 'µs'
}

export function outputUnitLabel(model: PwmModel | undefined): string {
  const unit = outputUnits(model)
  return unit === 'thr' ? 'throttle' : unit
}

export function activeProtocolSummary(status: ActuatorStatus): string {
  const motors = activeMotorCount(status.output_map)
  const labels = status.pwm_models
    .slice(0, motors)
    .map((model) => protocolLabel(model.protocol))
    .filter((label, index, all) => all.indexOf(label) === index)
  return labels.length > 0 ? labels.join(' + ') : '—'
}

export function outputConfigurationIssue(status: ActuatorStatus): string | null {
  if (!status.target_connected) return 'Connect the serial PX4 target'
  const motors = activeMotorCount(status.output_map)
  if (status.output_map.state !== 'complete' || motors === 0) {
    return 'Output mapping is not complete'
  }
  if (status.output_map.issues.length > 0) return 'Output mapping has assignment issues'
  const models = status.pwm_models.slice(0, motors)
  if (models.length < motors) return 'Output models are not loaded'
  const unread = models.findIndex((model) => model.range_source !== 'fc')
  if (unread >= 0) return `M${unread + 1} output range was not read from PX4`
  const unknown = models.findIndex((model) => model.protocol.kind === 'unknown')
  if (unknown >= 0) return `M${unknown + 1} output protocol is unknown`
  if (models.some((model) => isDShot(model.protocol)) && status.dshot_3d_enable !== 0) {
    return status.dshot_3d_enable == null
      ? 'DSHOT_3D_ENABLE was not read'
      : `DSHOT_3D_ENABLE=${status.dshot_3d_enable} blocks this output model`
  }
  return null
}

export interface ActuatorStatus {
  motor_control: { enabled: boolean }
  tones: MotorToneStatus
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
  target_origin: 'dedicated' | 'main_serial' | null
  main_serial_target: {
    target: { connection_id: number; target_system_id: number; target_component_id: number }
    transport_generation: number
    endpoint: string
  } | null
  /** COM_MOT_TEST_EN read from the target PX4 (null = unknown). */
  com_mot_test_en: number | null
  /** SDLOG_MODE read from the target PX4 (null = unknown), PX4 v1.17:
   * 0=armed until disarm, 1=boot until disarm, 2=boot until shutdown,
   * 3=AUX1 switch, 4=first arm until shutdown. */
  sdlog_mode: number | null
  /** SDLOG_BACKEND read from the target PX4 (null = unknown); bitmask:
   * 1=SD card, 2=MAVLink. */
  sdlog_backend: number | null
  parameter_restart_pending: boolean
  /** Normalized SITL servo outputs 1–4 (the forward INPUT). */
  input_values: number[]
  /** The SITL's raw SERVO_OUTPUT_RAW 1–4 in µs (what input_values came from). */
  input_pwm: number[]
  /** Values sent to the FC as ACTUATOR_TEST (the OUTPUT), 0 when idle. */
  output_values: number[]
  /** Converted PWM from the sender's actual model, not measured feedback.
   * null marks no active command for that motor. */
  output_pwm: (number | null)[]
  /** Actual PWM for each motor function in µs, selected from its mapped
   * MAIN/AUX channel; null = unmapped or no fresh sample. */
  fc_output_pwm: (number | null)[]
  /** Motor function → physical output mapping read from PWM_*_FUNCx. */
  output_map: OutputMap
  /** Per-motor target PWM model used to compensate ACTUATOR_TEST and
   * forwarded SITL pulses (endpoints, reversal, THR_MDL_FAC, disabled). */
  pwm_models: PwmModel[]
  /** Complete units/mapping descriptors from the same backend snapshot. */
  scope_outputs: ScopeOutputMeta[]
  /** PX4 output protocol for MAIN and AUX, from PWM_*_TIMn. */
  output_protocols: OutputProtocols
  /** DSHOT_3D_ENABLE (null = unread); nonzero blocks DShot starts. */
  dshot_3d_enable: number | null
  /** DSHOT_BIDIR_EN (null = unread), needed for ESC RPM telemetry. */
  dshot_bidir_en: number | null
  /** Per-ESC RPM from ESC_STATUS; null rpm = that ESC never reported. */
  esc_rpm: EscRpmStatus[]
  /** Target-shell SD-file capture state (Task 6). */
  sd_capture: SdCaptureStatus
  /** Per-test run coordinator + recovery manifest (Task 8). */
  run: RunSnapshot
}

export interface EscRpmStatus {
  esc: number
  rpm: number | null
  age_s: number | null
  stale: boolean
}

export type RunPhase =
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

export interface RunStatus {
  test_id: string
  phase: RunPhase
  remote_path: string | null
  stop_reason: string | null
  error: { kind: string; [key: string]: unknown } | null
}

export interface RunSnapshot {
  status: RunStatus | null
  manifest_path: string | null
}

export type CapturePhase =
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

export interface SdCaptureError {
  kind: string
  [key: string]: unknown
}

export interface SdCaptureStatus {
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

export type PwmRangeSource = 'fc' | 'fallback'

export interface PwmModel {
  /** PWM/OneShot: µs. DShot/BDShot: mixer units 0..1999. */
  min_us: number
  max_us: number
  reversed: boolean
  thrust_factor: number
  disabled: boolean
  range_source: PwmRangeSource
  /** PX4 output protocol detected for this motor's bank. */
  protocol: OutputProtocol
  /** PX4 DSHOT_MIN (0..1); unused for PWM/OneShot. */
  dshot_min: number
}

export type ServoPort = 'main' | 'aux'

export interface ServoChannel {
  port: ServoPort
  channel: number
}

export interface OutputAssignment {
  motor: number
  channel: ServoChannel | null
}

export interface MappingIssue {
  kind: 'unreadable' | 'duplicate'
  port?: ServoPort
  channel?: number
  motor?: number
  first?: ServoChannel
  second?: ServoChannel
}

export interface OutputMap {
  motors: OutputAssignment[]
  state: 'complete' | 'partial' | 'invalid' | 'unknown'
  issues: MappingIssue[]
}

export interface SitlSourceConfig {
  endpoint: string
  system_id: number
  component_id: number
  port: ServoPort
}

export function channelLabel(channel: ServoChannel): string {
  return `${channel.port === 'main' ? 'MAIN' : 'AUX'}${channel.channel}`
}

export function assignmentFor(map: OutputMap, motor: number): OutputAssignment | undefined {
  return map.motors.find((assignment) => assignment.motor === motor)
}

export const IDLE: ActuatorStatus = {
  motor_control: { enabled: false },
  tones: IDLE_TONES,
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
  target_origin: null,
  main_serial_target: null,
  com_mot_test_en: null,
  sdlog_mode: null,
  sdlog_backend: null,
  parameter_restart_pending: false,
  input_values: [],
  input_pwm: [],
  output_values: [],
  output_pwm: [],
  fc_output_pwm: [],
  output_map: { motors: [], state: 'unknown', issues: [] },
  pwm_models: [],
  scope_outputs: [],
  output_protocols: { main: { kind: 'unknown' }, aux: { kind: 'unknown' } },
  dshot_3d_enable: null,
  dshot_bidir_en: null,
  esc_rpm: [],
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
