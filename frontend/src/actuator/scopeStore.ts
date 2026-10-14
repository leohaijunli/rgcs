// Sample store for the motor-test scope: signal definitions (what the left
// list shows), one ring buffer per signal, per-signal detection stats, and
// the intervals during which the motor test was sending.
//
// Samples arrive from the shell as `actuator_samples` batches of
// `{ id, t, v }` (t = host epoch seconds). The ids are documented in
// `core::motor_test::scope`. A signal exists here only once its first sample
// arrives — which is how "is there an RPM / current signal" gets answered.

/** Which stacked plot pane a signal is drawn in. */
export type ScopeGroup = 'cmd' | 'mag' | 'esc'

export type ScopeUnit = 'µs' | '%' | 'nT' | 'rpm' | 'A' | 'V'

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
const DASH_FC = [6, 4]
const DASH_CMD = [2, 3]
const DASH_CURRENT = [6, 4]
const DASH_VOLT = [2, 3]

function perMotor(
  prefix: string,
  field: string,
  group: ScopeGroup,
  unit: ScopeUnit,
  label: string,
  dash: number[] | null,
  defaultOn: boolean,
): SignalDef[] {
  return MOTORS.map((m) => ({
    id: `${prefix}${m + 1}${field}`,
    group,
    label: `${label} ${m + 1}`,
    unit,
    motor: m,
    dash,
    defaultOn,
  }))
}

const DEFS: SignalDef[] = [
  ...perMotor('sitl.servo', '', 'cmd', 'µs', 'SITL servo', null, true),
  ...perMotor('fwd.cmd', '', 'cmd', '%', 'Sent cmd', DASH_CMD, false),
  ...perMotor('fc.servo', '', 'cmd', 'µs', 'FC servo', DASH_FC, true),
  {
    id: 'mag.raw_total',
    group: 'mag',
    label: '|B| raw',
    unit: 'nT',
    motor: null,
    dash: null,
    defaultOn: true,
  },
  {
    id: 'mag.cal_total',
    group: 'mag',
    label: '|B| calibrated',
    unit: 'nT',
    motor: null,
    dash: null,
    defaultOn: true,
  },
  ...perMotor('esc', '.rpm', 'esc', 'rpm', 'ESC rpm', null, true),
  ...perMotor('esc', '.current', 'esc', 'A', 'ESC current', DASH_CURRENT, true),
  ...perMotor('esc', '.voltage', 'esc', 'V', 'ESC voltage', DASH_VOLT, false),
]

const DEF_BY_ID = new Map(DEFS.map((d) => [d.id, d]))

/** Definition for `id`; ESC 5–8 (and anything else well-formed) is built on
 * demand so a bigger airframe shows up without a code change. */
export function defOf(id: string): SignalDef | undefined {
  const known = DEF_BY_ID.get(id)
  if (known) return known
  const m = /^esc(\d+)\.(rpm|current|voltage)$/.exec(id)
  if (!m) return undefined
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
  }
  DEF_BY_ID.set(id, def)
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
    title: 'FC servo (serial SERVO_OUTPUT_RAW)',
    ids: ids('fc.servo'),
    emptyHint: (c) =>
      c.targetConnected ? 'Waiting for SERVO_OUTPUT_RAW…' : 'Connect the PX4 serial link.',
  },
  {
    key: 'cmd',
    title: 'Sent to FC (ACTUATOR_TEST)',
    ids: ids('fwd.cmd'),
    emptyHint: () => 'Appears while a test is sending.',
  },
  {
    key: 'mag',
    title: 'Total field',
    ids: ['mag.raw_total', 'mag.cal_total'],
    emptyHint: (c) =>
      c.targetConnected ? 'Waiting for HIGHRES_IMU…' : 'Connect the PX4 serial link.',
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
}

export type SignalState = 'none' | 'zero' | 'live' | 'stale'

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
  v: number
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

  /** Ingest one batch. Out-of-order samples within a signal are dropped so
   * every ring stays sorted (the plot's merge relies on it). */
  ingest(batch: IncomingSample[]): void {
    for (const s of batch) {
      if (!Number.isFinite(s.t)) continue
      if (s.id === RUN_ID) {
        this.markRun(s.v > 0.5, s.t)
        continue
      }
      if (!Number.isFinite(s.v) || !defOf(s.id)) continue
      let ring = this.rings.get(s.id)
      if (!ring) {
        ring = new ScopeRing(RING_CAP)
        this.rings.set(s.id, ring)
      }
      const n = ring.length
      if (n > 0 && s.t < ring.timeAt(n - 1)) continue
      ring.push(s.t, s.v)
      if (s.t > this.newestT) this.newestT = s.t

      let st = this.stats.get(s.id)
      if (!st) {
        st = { count: 0, last: 0, lastT: s.t, rateHz: 0, everNonZero: false }
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
      st.last = s.v
      if (s.v !== 0) st.everNonZero = true

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
    }
    // Bounded history.
    if (this.runs.length > 64) this.runs.splice(0, this.runs.length - 64)
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
    if (nowT - st.lastT > STALE_S) return 'stale'
    return st.everNonZero ? 'live' : 'zero'
  }

  /** Drop the data (keeps the auto-check memory: a cleared signal that the
   * operator had unchecked does not pop back). */
  clear(): void {
    this.rings.clear()
    this.stats.clear()
    this.runs = []
    this.newestT = 0
  }
}

/** Process-wide store: lives for the window's lifetime so data keeps
 * accumulating while the Controls tab is shown. */
export const scopeStore = new ScopeStore()
