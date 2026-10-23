// Shared motor-bench status contracts. These mirror the Rust payloads from
// `ActuatorStatus`; keep them beside the window so the tracking table and
// header do not reach into the panel component.

/** Motor count shown by the bench (fixed 4 until a vehicle config lands). */
export const MOTOR_COUNT = 4

export interface ActuatorStatus {
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
  parameter_restart_pending: boolean
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

export interface PwmModel {
  min_us: number
  max_us: number
  reversed: boolean
  thrust_factor: number
  disabled: boolean
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
  parameter_restart_pending: false,
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
