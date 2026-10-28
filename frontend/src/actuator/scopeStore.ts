// Sample store for the motor-test scope: signal definitions (what the left
// list shows), one ring buffer per signal, per-signal detection stats, and
// the intervals during which the motor test was sending.
//
// Samples arrive from the shell as `actuator_samples` batches of
// `{ id, t, v, output? }` (t = host epoch seconds). Output metadata carries
// configuration identity, physical channel and units. The ids are documented in
// `core::motor_test::scope`. A signal exists here only once its first sample
// arrives — which is how "is there an RPM / current signal" gets answered.

/** Signal category in the list; either plot can draw signals from any category. */
export type ScopeGroup = 'cmd' | 'mag' | 'esc' | 'bat'

export type ScopeUnit = 'µs' | 'thr' | 'raw' | 'nT' | 'rpm' | 'A' | 'V'
export type ScopeGapReason = 'released' | 'stale' | 'run_boundary'
const TRACE_COLOR_RE = /^#[0-9a-f]{6}$/i

export function normalizeTraceColor(value: unknown): `#${string}` | null {
  return typeof value === 'string' && TRACE_COLOR_RE.test(value)
    ? value.toLowerCase() as `#${string}`
    : null
}

export interface ScopeOutputMeta {
  connection_id: number
  revision: number
  motor: number
  channel: { port: 'main' | 'aux'; channel: number }
  unit: 'microseconds' | 'd_shot_throttle'
}

export type ScopeConfigResult =
  | { ok: true }
  | { ok: false; reason: 'invalid_configuration' | 'obsolete_configuration' }

export interface SignalDef {
  id: string
  group: ScopeGroup
  /** Short label (list row, legend). */
  label: string
  unit: ScopeUnit
  /** 0-based motor index for colouring; null = not per-motor. */
  motor: number | null
  /** Line dash pattern distinguishing source/quantity at equal colour. */
  dash: number[] | null
  /** Auto-checked the first time data for it appears. */
  defaultOn: boolean
  /** Seconds without a value before one stale gap is inserted; null = explicit gaps only. */
  staleAfterS: number | null
  /** Output configuration identity; included in the mounted plot's identity. */
  contextKey?: string
  /** Optional design-token override for a non-motor source. */
  defaultColorToken?: string
}

/** One list section (a collapsible group of related signals). */
export interface ListGroup {
  key: string
  title: string
  /** Shown while none of the section's signals has data yet. */
  emptyHint: (ctx: HintContext) => string | null
  ids: string[]
}

export interface HintContext {
  sitlConnected: boolean
  targetConnected: boolean
  calLoaded: boolean
}

const MOTORS = [0, 1, 2, 3]
const OUTPUT_CHANNEL_COUNT = 8
const OUTPUT_MOTOR_BY_ID = new Map<string, number>(
  MOTORS.flatMap((motor) => [
    [`cmd.pwm${motor + 1}`, motor + 1] as const,
    [`fc.motor${motor + 1}`, motor + 1] as const,
  ]),
)
const DASH_FC = [6, 4]
const DASH_CMD = [2, 3]
const DASH_CURRENT = [6, 4]
const DASH_VOLT = [2, 3]
const DEFAULT_STALE_S = 2
const ESC_STALE_S = 0.5

/** Stable backend/source identities from `core::motor_test::scope::MagScopeSource`. */
export const MAG_SCOPE_SOURCES = [
  { id: 'highres_imu', label: 'HIGHRES_IMU', color: '--mg-trace-total-mag' },
  { id: 'scaled_imu1', label: 'SCALED_IMU (IMU1)', color: '--mg-trace-1' },
  { id: 'scaled_imu2', label: 'SCALED_IMU2 (IMU2)', color: '--mg-trace-2' },
  { id: 'scaled_imu3', label: 'SCALED_IMU3 (IMU3)', color: '--mg-trace-3' },
] as const

export const MAG_RAW_IDS = MAG_SCOPE_SOURCES.map((source) => `mag.${source.id}.raw_total`)
export const MAG_CAL_IDS = MAG_SCOPE_SOURCES.map((source) => `mag.${source.id}.cal_total`)
export const MAG_SCOPE_IDS = MAG_SCOPE_SOURCES.flatMap((source) => [
  `mag.${source.id}.raw_total`,
  `mag.${source.id}.cal_total`,
])

function perMotor(
  prefix: string,
  field: string,
  group: ScopeGroup,
  unit: ScopeUnit,
  label: string,
  dash: number[] | null,
  defaultOn: boolean,
  staleAfterS: number | null,
): SignalDef[] {
  return MOTORS.map((m) => ({
    id: `${prefix}${m + 1}${field}`,
    group,
    label: `${label} ${m + 1}`,
    unit,
    motor: m,
    dash,
    defaultOn,
    staleAfterS,
  }))
}

const DEFS: SignalDef[] = [
  ...perMotor('sitl.servo', '', 'cmd', 'µs', 'SITL servo', null, true, DEFAULT_STALE_S),
  ...perMotor('cmd.pwm', '', 'cmd', 'raw', 'Command output M', DASH_CMD, true, null),
  ...perMotor('fc.motor', '', 'cmd', 'raw', 'FC output M', DASH_FC, true, DEFAULT_STALE_S),
  ...MAG_SCOPE_SOURCES.flatMap((source): SignalDef[] => [
    {
      id: `mag.${source.id}.raw_total`,
      group: 'mag',
      label: `${source.label} |B| raw`,
      unit: 'nT',
      motor: null,
      dash: DASH_FC,
      defaultOn: true,
      staleAfterS: DEFAULT_STALE_S,
      defaultColorToken: source.color,
    },
    {
      id: `mag.${source.id}.cal_total`,
      group: 'mag',
      label: `${source.label} |B| calibrated`,
      unit: 'nT',
      motor: null,
      dash: null,
      defaultOn: true,
      staleAfterS: DEFAULT_STALE_S,
      defaultColorToken: source.color,
    },
  ]),
  {
    id: 'bat1.current',
    group: 'bat',
    label: 'Battery current 1',
    unit: 'A',
    motor: 0,
    dash: null,
    defaultOn: true,
    staleAfterS: DEFAULT_STALE_S,
  },
  ...perMotor('esc', '.rpm', 'esc', 'rpm', 'ESC rpm', null, true, ESC_STALE_S),
  ...perMotor('esc', '.current', 'esc', 'A', 'ESC current', DASH_CURRENT, true, ESC_STALE_S),
  ...perMotor('esc', '.voltage', 'esc', 'V', 'ESC voltage', DASH_VOLT, false, ESC_STALE_S),
]

const DEF_BY_ID = new Map(DEFS.map((d) => [d.id, d]))

/** Definition for `id`; ESC 5–8 and extra battery ids are built on demand so a
 * bigger airframe / more batteries show up without a code change. */
export function defOf(id: string): SignalDef | undefined {
  return scopeStore.definitionOf(id)
}

function definitionFor(id: string, definitions: Map<string, SignalDef>): SignalDef | undefined {
  const known = definitions.get(id)
  if (known) return known
  const m = /^esc(\d+)\.(rpm|current|voltage)$/.exec(id)
  if (!m) {
    const b = /^bat(\d+)\.current$/.exec(id)
    if (!b) return undefined
    const k = Number(b[1])
    const def: SignalDef = {
      id,
      group: 'bat',
      label: `Battery current ${k}`,
      unit: 'A',
      motor: (k - 1) % 4,
      dash: null,
      defaultOn: true,
      staleAfterS: DEFAULT_STALE_S,
    }
    definitions.set(id, def)
    return def
  }
  const k = Number(m[1])
  const q = m[2] as 'rpm' | 'current' | 'voltage'
  const def: SignalDef = {
    id,
    group: 'esc',
    label: `ESC ${q} ${k}`,
    unit: q === 'rpm' ? 'rpm' : q === 'current' ? 'A' : 'V',
    motor: (k - 1) % 4,
    dash: q === 'rpm' ? null : q === 'current' ? DASH_CURRENT : DASH_VOLT,
    defaultOn: q !== 'voltage',
    staleAfterS: ESC_STALE_S,
  }
  definitions.set(id, def)
  return def
}

const ids = (prefix: string, suffix = '') =>
  MOTORS.map((m) => `${prefix}${m + 1}${suffix}`)

/** The left list's sections, in display order. */
export const LIST_GROUPS: ListGroup[] = [
  {
    key: 'sitl',
    title: 'SITL servo (SERVO_OUTPUT_RAW)',
    ids: ids('sitl.servo'),
    emptyHint: (c) => (c.sitlConnected ? 'Waiting for SERVO_OUTPUT_RAW…' : 'Connect the SITL source.'),
  },
  {
    key: 'fc',
    title: 'Mapped FC outputs (SERVO_OUTPUT_RAW)',
    ids: ids('fc.motor'),
    emptyHint: (c) =>
      c.targetConnected ? 'Waiting for SERVO_OUTPUT_RAW…' : 'Connect the PX4 serial link.',
  },
  {
    key: 'cmd',
    title: 'Command output (converted ACTUATOR_TEST)',
    ids: ids('cmd.pwm'),
    emptyHint: () => 'Appears while a test is sending.',
  },
  {
    key: 'mag',
    title: 'Total field by IMU source (nT)',
    ids: MAG_SCOPE_IDS,
    emptyHint: (c) =>
      c.targetConnected ? 'Waiting for HIGHRES_IMU or SCALED_IMU streams…' : 'Connect the PX4 serial link.',
  },
  {
    key: 'bat',
    title: 'Battery current (BATTERY_STATUS)',
    ids: ['bat1.current'],
    emptyHint: (c) =>
      c.targetConnected ? 'Waiting for BATTERY_STATUS…' : 'Connect the PX4 serial link.',
  },
  {
    key: 'rpm',
    title: 'ESC rpm',
    ids: ids('esc', '.rpm'),
    emptyHint: (c) =>
      c.targetConnected
        ? 'No ESC_STATUS received: needs an ESC that reports telemetry (DShot telemetry / DroneCAN).'
        : 'Connect the PX4 serial link.',
  },
  {
    key: 'current',
    title: 'ESC current',
    ids: ids('esc', '.current'),
    emptyHint: () => null,
  },
  {
    key: 'voltage',
    title: 'ESC voltage',
    ids: ids('esc', '.voltage'),
    emptyHint: () => null,
  },
]

/** Preallocated two-channel ring (time, value). */
export class ScopeRing {
  private t: Float64Array
  private v: Float64Array
  private start = 0
  private size = 0

  constructor(readonly cap: number) {
    this.t = new Float64Array(cap)
    this.v = new Float64Array(cap)
  }

  get length(): number {
    return this.size
  }

  push(t: number, v: number): void {
    if (this.size > 0 && t === this.timeAt(this.size - 1)) {
      this.v[(this.start + this.size - 1) % this.cap] = v
      return
    }
    const i = (this.start + this.size) % this.cap
    this.t[i] = t
    this.v[i] = v
    if (this.size < this.cap) this.size++
    else this.start = (this.start + 1) % this.cap
  }

  timeAt(i: number): number {
    return this.t[(this.start + i) % this.cap]
  }

  valueAt(i: number): number {
    return this.v[(this.start + i) % this.cap]
  }

  /** Index of the first sample with time >= cutoff (times are increasing). */
  firstAtOrAfter(cutoff: number): number {
    let lo = 0
    let hi = this.size
    while (lo < hi) {
      const mid = (lo + hi) >> 1
      if (this.timeAt(mid) < cutoff) lo = mid + 1
      else hi = mid
    }
    return lo
  }

  clear(): void {
    this.start = 0
    this.size = 0
  }
}

/** Per-signal detection state for the list's status dot. */
export interface SignalStat {
  count: number
  last: number
  lastT: number
  /** EMA of the arrival rate, Hz. */
  rateHz: number
  /** False until a non-zero value has been seen: an ESC that answers with
   * exact zeros has no current/voltage telemetry, which is not "idle". */
  everNonZero: boolean
  /** Latest intentional break after `lastT`; cleared by the next value. */
  lastGap: ScopeGapReason | null
}

export type SignalState = 'none' | 'zero' | 'live' | 'stale' | 'released' | 'boundary'

/** Seconds without a sample before a seen signal counts as stale. */
export const STALE_S = 2
const RING_CAP = 20_000
const RATE_ALPHA = 0.2
/** Marker pseudo-signal: 1 = the motor test is sending, 0 = stopped. */
const RUN_ID = 'session.running'

export interface RunInterval {
  t0: number
  /** null = still running. */
  t1: number | null
}

export interface IncomingSample {
  id: string
  t: number
  v: number | null
  gap?: ScopeGapReason | null
  output?: ScopeOutputMeta | null
}

function validGapReason(value: unknown): value is ScopeGapReason {
  return value === 'released' || value === 'stale' || value === 'run_boundary'
}

function validOutputMeta(value: unknown): value is ScopeOutputMeta {
  if (typeof value !== 'object' || value === null) return false
  const output = value as Partial<ScopeOutputMeta>
  return typeof output.connection_id === 'number'
    && Number.isSafeInteger(output.connection_id) && output.connection_id >= 0
    && typeof output.revision === 'number'
    && Number.isSafeInteger(output.revision) && output.revision > 0
    && typeof output.motor === 'number'
    && Number.isInteger(output.motor) && output.motor >= 1 && output.motor <= MOTORS.length
    && typeof output.channel === 'object' && output.channel !== null
    && (output.channel.port === 'main' || output.channel.port === 'aux')
    && Number.isInteger(output.channel.channel)
    && output.channel.channel >= 1 && output.channel.channel <= OUTPUT_CHANNEL_COUNT
    && (output.unit === 'microseconds' || output.unit === 'd_shot_throttle')
}

function sameOutputMeta(a: ScopeOutputMeta, b: ScopeOutputMeta): boolean {
  return a.connection_id === b.connection_id && a.revision === b.revision
    && a.motor === b.motor && a.unit === b.unit
    && a.channel.port === b.channel.port && a.channel.channel === b.channel.channel
}

export class ScopeStore {
  readonly rings = new Map<string, ScopeRing>()
  readonly stats = new Map<string, SignalStat>()
  runs: RunInterval[] = []
  /** Ids whose first appearance has already been offered for auto-check. */
  private offered = new Set<string>()
  private pendingNew: string[] = []
  /** Newest sample time seen across all signals (host epoch seconds). */
  newestT = 0
  /** Epoch of the first accepted plotted value; the visible x-axis starts at 0 s. */
  originT: number | null = null
  private definitions = new Map(DEF_BY_ID)
  private outputs = new Map<number, ScopeOutputMeta>()
  private lastContext: { connection_id: number; revision: number } | null = null
  private lastConfigSignature = ''

  definitionOf(id: string): SignalDef | undefined {
    return definitionFor(id, this.definitions)
  }

  /** Configure units from one published backend snapshot. Data cannot infer or
   * replace configuration. Older snapshots and conflicting metadata are refused. */
  configureOutputs(outputs: readonly ScopeOutputMeta[]): ScopeConfigResult {
    if (!Array.isArray(outputs) || outputs.length > MOTORS.length || !outputs.every(validOutputMeta)) {
      return { ok: false, reason: 'invalid_configuration' }
    }
    if (outputs.length === 0) {
      this.outputs.clear()
      this.dropOutputHistory()
      this.updateOutputDefinitions([])
      return { ok: true }
    }
    const next = outputs.map((output) => ({
      connection_id: output.connection_id,
      revision: output.revision,
      motor: output.motor,
      channel: { port: output.channel.port, channel: output.channel.channel },
      unit: output.unit,
    })).sort((a, b) => a.motor - b.motor)
    const first = next[0]
    const channels = new Set(next.map((output) => `${output.channel.port}:${output.channel.channel}`))
    if (channels.size !== next.length || next.some((output, index) =>
      output.motor !== index + 1 || output.connection_id !== first.connection_id || output.revision !== first.revision,
    )) return { ok: false, reason: 'invalid_configuration' }
    const signature = JSON.stringify(next)
    if (this.lastContext) {
      if (first.connection_id < this.lastContext.connection_id
        || (first.connection_id === this.lastContext.connection_id && first.revision < this.lastContext.revision)) {
        return { ok: false, reason: 'obsolete_configuration' }
      }
      if (first.connection_id === this.lastContext.connection_id && first.revision === this.lastContext.revision
        && signature !== this.lastConfigSignature) {
        return { ok: false, reason: 'invalid_configuration' }
      }
    }
    if (this.outputs.size > 0 && signature === this.lastConfigSignature) return { ok: true }
    this.dropOutputHistory()
    this.outputs = new Map(next.map((output) => [output.motor, output]))
    this.lastContext = { connection_id: first.connection_id, revision: first.revision }
    this.lastConfigSignature = signature
    this.updateOutputDefinitions(next)
    return { ok: true }
  }

  private dropOutputHistory(): void {
    for (const id of OUTPUT_MOTOR_BY_ID.keys()) {
      this.rings.delete(id)
      this.stats.delete(id)
    }
  }

  private updateOutputDefinitions(outputs: readonly ScopeOutputMeta[]): void {
    const byMotor = new Map(outputs.map((output) => [output.motor, output]))
    for (const [id, motor] of OUTPUT_MOTOR_BY_ID) {
      const base = DEF_BY_ID.get(id)
      if (!base) continue
      const output = byMotor.get(motor)
      const unit: ScopeUnit = output ? (output.unit === 'microseconds' ? 'µs' : 'thr') : 'raw'
      const contextKey = output ? JSON.stringify(output) : ''
      const label = id.startsWith('cmd.pwm')
        ? `${unit === 'thr' ? 'Command throttle' : unit === 'µs' ? 'Command PWM' : 'Command output'} M ${motor}`
        : `FC output M ${motor}${output ? ` (${output.channel.port.toUpperCase()}${output.channel.channel})` : ''}`
      const current = this.definitions.get(id)
      if (current?.contextKey === contextKey && current.unit === unit && current.label === label) continue
      this.definitions.set(id, { ...base, label, unit, contextKey })
    }
  }

  /** Ingest one batch. Out-of-order samples within a signal are dropped so
   * every ring stays sorted (the plot's merge relies on it). */
  ingest(batch: IncomingSample[]): void {
    for (const s of batch) {
      if (!Number.isFinite(s.t)) continue
      if (s.id === RUN_ID) {
        this.markRun((s.v ?? 0) > 0.5, s.t)
        continue
      }
      const motor = OUTPUT_MOTOR_BY_ID.get(s.id)
      if (motor !== undefined) {
        const expected = this.outputs.get(motor)
        if (!expected || !validOutputMeta(s.output) || !sameOutputMeta(s.output, expected)) continue
      }
      let ring = this.rings.get(s.id)
      if (s.v === null) {
        // A valid gap never creates a signal that has not produced a value.
        if (ring && validGapReason(s.gap)) this.insertGap(s.id, s.t, s.gap)
        continue
      }
      if (!Number.isFinite(s.v) || !this.definitionOf(s.id)) continue
      if (!ring) {
        ring = new ScopeRing(RING_CAP)
        this.rings.set(s.id, ring)
      }
      if (this.originT === null) this.originT = s.t
      const n = ring.length
      if (n > 0 && s.t < ring.timeAt(n - 1)) continue
      const value = s.v
      ring.push(s.t, value)
      if (s.t > this.newestT) this.newestT = s.t

      let st = this.stats.get(s.id)
      if (!st) {
        st = { count: 0, last: 0, lastT: s.t, rateHz: 0, everNonZero: false, lastGap: null }
        this.stats.set(s.id, st)
      } else {
        const dt = s.t - st.lastT
        if (dt > 1e-6) {
          const inst = 1 / dt
          st.rateHz = st.rateHz === 0 ? inst : RATE_ALPHA * inst + (1 - RATE_ALPHA) * st.rateHz
        }
        st.lastT = s.t
      }
      st.count++
      st.last = value
      st.lastGap = null
      if (Number.isFinite(value) && value !== 0) st.everNonZero = true

      if (!this.offered.has(s.id)) {
        this.offered.add(s.id)
        this.pendingNew.push(s.id)
      }
    }
  }

  private markRun(running: boolean, t: number): void {
    const open = this.runs.length > 0 ? this.runs[this.runs.length - 1] : null
    if (running && (!open || open.t1 !== null)) {
      this.runs.push({ t0: t, t1: null })
    } else if (!running && open && open.t1 === null) {
      open.t1 = t
      for (const id of this.rings.keys()) {
        if (/^(?:cmd\.pwm|fc\.motor)\d+$/.test(id) || /^esc\d+\.rpm$/.test(id)) {
          this.insertGap(id, t, 'run_boundary')
        }
      }
    }
    // Bounded history.
    if (this.runs.length > 64) this.runs.splice(0, this.runs.length - 64)
  }

  private insertGap(id: string, t: number, reason: ScopeGapReason): void {
    const ring = this.rings.get(id)
    const stat = this.stats.get(id)
    if (!ring || !stat || !Number.isFinite(t)) return
    const n = ring.length
    if (n > 0 && t < ring.timeAt(n - 1)) return
    if (stat.lastGap === reason && n > 0 && !Number.isFinite(ring.valueAt(n - 1))) return
    ring.push(t, NaN)
    stat.lastGap = reason
    if (t > this.newestT) this.newestT = t
  }

  /** Insert one deterministic stale boundary for every overdue live series. */
  advanceTime(nowT: number): void {
    if (!Number.isFinite(nowT)) return
    for (const [id, stat] of this.stats) {
      const staleAfterS = this.definitionOf(id)?.staleAfterS
      if (staleAfterS == null || stat.lastGap !== null || nowT - stat.lastT <= staleAfterS) continue
      this.insertGap(id, stat.lastT + staleAfterS, 'stale')
    }
  }

  /** Mark ids as already offered for auto-check (restored from the previous
   * session, so a signal the operator unchecked does not pop back). */
  preloadOffered(ids: string[]): void {
    for (const id of ids) this.offered.add(id)
  }

  /** Signals seen for the first time since the last call (auto-check). */
  takeNew(): string[] {
    const out = this.pendingNew
    this.pendingNew = []
    return out
  }

  stateOf(id: string, nowT: number): SignalState {
    const st = this.stats.get(id)
    if (!st) return 'none'
    if (st.lastGap === 'released') return 'released'
    if (st.lastGap === 'run_boundary') return 'boundary'
    if (st.lastGap === 'stale') return 'stale'
    const staleAfterS = this.definitionOf(id)?.staleAfterS ?? STALE_S
    if (nowT - st.lastT > staleAfterS) return 'stale'
    return st.everNonZero ? 'live' : 'zero'
  }

  elapsedAt(epochSeconds: number): number {
    if (this.originT === null || !Number.isFinite(epochSeconds)) return 0
    return Math.max(0, epochSeconds - this.originT)
  }

  /** Drop the data (keeps the auto-check memory: a cleared signal that the
   * operator had unchecked does not pop back). */
  clear(): void {
    this.rings.clear()
    this.stats.clear()
    this.runs = []
    this.newestT = 0
    this.originT = null
  }
}

/** Process-wide store: lives for the window's lifetime so data keeps
 * accumulating while the Controls tab is shown. */
export const scopeStore = new ScopeStore()
