// Signal scope for the motor-test window. Layout follows the Signal
// Inspector: a signal list with checkboxes on the left, plots on the right.
//
//   - SITL servo 1–4 (µs) and the FC servo 1–4 parsed from the serial link
//   - converted PWM from the sender's ACTUATOR_TEST command (µs)
//   - total field before / after calibration (nT)
//   - total battery current from BATTERY_STATUS (A)
//   - ESC rpm / current / voltage — listed as soon as ESC_STATUS arrives
//
// Checked signals are drawn in stacked panes (commands / field / ESC /
// battery) that share one rolling time window and one cursor. Colour = motor
// number, dash = source or quantity, so SITL vs serial for the same motor line
// up.

import { useCallback, useEffect, useMemo, useRef, useState, type MutableRefObject } from 'react'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'
import { listen } from '@tauri-apps/api/event'
import { cssVar } from '../design-system/theme'
import { isTauri } from '../inspector/mock'
import {
  LIST_GROUPS,
  defOf,
  scopeStore,
  type HintContext,
  type IncomingSample,
  type ScopeGroup,
  type SignalDef,
  type SignalState,
} from './scopeStore'

/** Plot redraw cadence; the shell flushes batches at ~20 Hz. */
const REDRAW_MS = 100
/** List refresh cadence (status dots, last values). */
const LIST_MS = 500
const WINDOWS_S = [10, 30, 60, 120]
const DEFAULT_WINDOW_S = 30
const STORE_KEY = 'maggcs.actuator.scope'
const SYNC_KEY = 'maggcs-motor-scope'

const PANES: Array<{ group: ScopeGroup; title: string }> = [
  { group: 'cmd', title: 'Servo commands' },
  { group: 'mag', title: 'Total field' },
  { group: 'esc', title: 'ESC' },
  { group: 'bat', title: 'Battery' },
]

// ---------------------------------------------------------------- feed ---

/** Feed the store from the shell's `actuator_samples` events for as long as
 * the window lives (so history accumulates while another tab is shown).
 * Outside Tauri (browser dev) a synthetic generator stands in. */
export function useScopeFeed(): void {
  useEffect(() => {
    if (!isTauri()) return startMock()
    let disposed = false
    let un: (() => void) | undefined
    void (async () => {
      try {
        un = await listen<IncomingSample[]>('actuator_samples', (e) => {
          if (!disposed) scopeStore.ingest(e.payload)
        })
      } catch {
        /* events unavailable: the scope stays empty */
      }
    })()
    return () => {
      disposed = true
      un?.()
    }
  }, [])
}

function startMock(): () => void {
  const t0 = Date.now() / 1000
  const id = window.setInterval(() => {
    const t = Date.now() / 1000
    const x = t - t0
    const ramp = Math.min(1, Math.max(0, Math.sin(x / 6) * 1.5))
    const running = ramp > 0.05
    const batch: IncomingSample[] = []
    for (let m = 0; m < 4; m++) {
      const us = 1000 + 1000 * ramp * (0.85 + 0.05 * m)
      batch.push({ id: `sitl.servo${m + 1}`, t, v: us })
      batch.push({ id: `fc.servo${m + 1}`, t: t + 0.01, v: us - 4 })
      batch.push({ id: `cmd.pwm${m + 1}`, t, v: running ? 1100 + 800 * ramp * (0.85 + 0.05 * m) : null })
      batch.push({ id: `esc${m + 1}.rpm`, t, v: 9000 * ramp * (0.9 + 0.03 * m) })
      batch.push({ id: `esc${m + 1}.current`, t, v: 12 * ramp * ramp })
    }
    const raw = 50_000 + 40 * Math.sin(x * 3) + 600 * ramp
    batch.push({ id: 'mag.raw_total', t, v: raw })
    batch.push({ id: 'mag.cal_total', t, v: raw - 2000 - 300 * ramp })
    batch.push({ id: 'bat1.current', t, v: 12 * ramp * ramp })
    batch.push({ id: 'session.running', t, v: running ? 1 : 0 })
    scopeStore.ingest(batch)
  }, 50)
  return () => window.clearInterval(id)
}

// ------------------------------------------------------------- persist ---

interface Persisted {
  checked: string[]
  seen: string[]
  windowS: number
}

function loadPersisted(): Persisted {
  try {
    const raw = localStorage.getItem(STORE_KEY)
    if (raw) {
      const p = JSON.parse(raw) as Partial<Persisted>
      return {
        checked: Array.isArray(p.checked) ? p.checked.filter((x) => typeof x === 'string').map((x) => x.replace(/^fwd\.cmd([1-4])$/, 'cmd.pwm$1')) : [],
        seen: Array.isArray(p.seen) ? p.seen.filter((x) => typeof x === 'string').map((x) => x.replace(/^fwd\.cmd([1-4])$/, 'cmd.pwm$1')) : [],
        windowS: WINDOWS_S.includes(p.windowS ?? -1) ? (p.windowS as number) : DEFAULT_WINDOW_S,
      }
    }
  } catch {
    /* corrupt or unavailable storage: start fresh */
  }
  return { checked: [], seen: [], windowS: DEFAULT_WINDOW_S }
}

function savePersisted(p: Persisted): void {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(p))
  } catch {
    /* storage full / unavailable: the scope still works */
  }
}

// -------------------------------------------------------------- colour ---

function colorOf(def: SignalDef): string {
  if (def.motor !== null) return cssVar(`--mg-trace-${def.motor + 1}`)
  return def.id === 'mag.cal_total' ? cssVar('--mg-trace-total-mag') : cssVar('--mg-trace-5')
}

/** `#rrggbb` or a CSS token + alpha → `color-mix` at the requested alpha. */
function withAlpha(color: string, alpha: number): string {
  return `color-mix(in srgb, ${color} ${alpha * 100}%, transparent)`
}

// ----------------------------------------------------------- component ---

export default function SignalScope({
  sitlConnected,
  targetConnected,
  calLoaded,
  legendOpen = true,
  onToggleLegend,
}: HintContext & { legendOpen?: boolean; onToggleLegend?: () => void }) {
  const initial = useMemo(loadPersisted, [])
  const [checked, setChecked] = useState<Set<string>>(() => new Set(initial.checked))
  const [windowS, setWindowS] = useState(initial.windowS)
  const [paused, setPaused] = useState(false)
  const [appearance, setAppearance] = useState(0)
  const [, setTick] = useState(0)
  const seenRef = useRef<Set<string>>(new Set(initial.seen))
  const checkedRef = useRef(checked)
  checkedRef.current = checked
  const pausedAtRef = useRef<number | null>(null)
  const windowRef = useRef(windowS)
  windowRef.current = windowS

  // uPlot resolves its canvas colors at construction. Rebuild only the plots
  // when the window's theme/density changes, keeping samples and selection.
  useEffect(() => {
    const observer = new MutationObserver(() => setAppearance((v) => v + 1))
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-theme', 'class'],
    })
    return () => observer.disconnect()
  }, [])

  // The store outlives this component; tell it what was already offered.
  useEffect(() => {
    scopeStore.preloadOffered(initial.seen)
  }, [initial.seen])

  // Persist the selection.
  useEffect(() => {
    savePersisted({ checked: [...checked], seen: [...seenRef.current], windowS })
  }, [checked, windowS])

  // List refresh + auto-check of newly detected signals.
  useEffect(() => {
    const id = window.setInterval(() => {
      // A signal is auto-checked only the first time it is ever seen (also
      // across sessions): one the operator unchecked stays unchecked.
      const fresh = scopeStore.takeNew().filter((f) => !seenRef.current.has(f))
      if (fresh.length > 0) {
        for (const f of fresh) seenRef.current.add(f)
        const add = fresh.filter((f) => defOf(f)?.defaultOn)
        if (add.length > 0) {
          setChecked((prev) => {
            const next = new Set(prev)
            for (const a of add) next.add(a)
            return next
          })
        } else {
          savePersisted({
            checked: [...checkedRef.current],
            seen: [...seenRef.current],
            windowS: windowRef.current,
          })
        }
      }
      setTick((n) => n + 1)
    }, LIST_MS)
    return () => window.clearInterval(id)
  }, [])

  const toggle = useCallback((ids: string[], on: boolean) => {
    setChecked((prev) => {
      const next = new Set(prev)
      for (const id of ids) {
        if (on) next.add(id)
        else next.delete(id)
      }
      return next
    })
  }, [])

  const togglePause = () => {
    if (paused) {
      pausedAtRef.current = null
      setPaused(false)
    } else {
      pausedAtRef.current = Date.now() / 1000
      setPaused(true)
    }
  }

  const clear = () => {
    scopeStore.clear()
  }

  const hint: HintContext = { sitlConnected, targetConnected, calLoaded }
  const nowT = Date.now() / 1000

  const paneDefs = (group: ScopeGroup): SignalDef[] => {
    const out: SignalDef[] = []
    for (const g of LIST_GROUPS) {
      for (const id of g.ids) {
        const d = defOf(id)
        if (d && d.group === group && checked.has(id)) out.push(d)
      }
    }
    // ESC 5–8 and other dynamic ids that are checked but not in the list.
    for (const id of checked) {
      const d = defOf(id)
      if (d && d.group === group && !out.includes(d)) out.push(d)
    }
    return out
  }
  const panes = PANES.map((p) => ({ ...p, defs: paneDefs(p.group) })).filter((p) => p.defs.length > 0)

  return (
    <div className={`scope-grid${legendOpen ? ' legend-open' : ''}`}>
      <SignalList checked={checked} hint={hint} nowT={nowT} onToggle={toggle} />
      <div className="scope-plots">
        <div className="scope-toolbar">
          {onToggleLegend && (
            <button
              className="mode-btn scope-btn"
              onClick={onToggleLegend}
              title={legendOpen ? 'Hide the signal list' : 'Show the signal list'}
              aria-label="Signal list"
              aria-expanded={legendOpen}
              aria-controls="motor-signal-list"
            >
              {legendOpen ? '◀ Signals' : 'Signals ▶'}
            </button>
          )}
          <span className="section-title">Window</span>
          <select
            className="scope-select"
            value={windowS}
            onChange={(e) => setWindowS(Number(e.target.value))}
            aria-label="Time window"
          >
            {WINDOWS_S.map((w) => (
              <option key={w} value={w}>
                {w} s
              </option>
            ))}
          </select>
          <button className={`mode-btn scope-btn ${paused ? 'active' : ''}`} onClick={togglePause}>
            {paused ? 'Resume' : 'Pause'}
          </button>
          <button className="mode-btn scope-btn" onClick={clear} title="Drop all buffered samples">
            Clear
          </button>
          <span className="scope-legend-note">
            <span className="run-swatch" /> motor test sending
          </span>
        </div>
        {panes.length === 0 ? (
          <div className="scope-empty">Check a signal on the left to plot it.</div>
        ) : (
          <div className="scope-panes">
            {panes.map((p) => (
              <ScopePane
                key={`${p.group}:${appearance}`}
                title={p.title}
                defs={p.defs}
                windowRef={windowRef}
                pausedAtRef={pausedAtRef}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- list ---

function SignalList({
  checked,
  hint,
  nowT,
  onToggle,
}: {
  checked: Set<string>
  hint: HintContext
  nowT: number
  onToggle: (ids: string[], on: boolean) => void
}) {
  /** Operator overrides of a group's open/closed state; absent = open when
   * the group has data (groups without data start collapsed). */
  const [openOverride, setOpenOverride] = useState<Record<string, boolean>>({})

  // ESC 5–8 (and other dynamic ids) that have data but are not in the static
  // groups get their own section so a larger airframe is still reachable.
  const extra = [...scopeStore.stats.keys()].filter(
    (id) => !LIST_GROUPS.some((g) => g.ids.includes(id)),
  )
  extra.sort((a, b) => a.localeCompare(b, undefined, { numeric: true }))

  return (
    <div className="signal-list" id="motor-signal-list">
      <div className="signal-list-title">Signals</div>
      {LIST_GROUPS.map((g) => {
        const states = g.ids.map((id) => scopeStore.stateOf(id, nowT))
        const present = states.filter((s) => s !== 'none').length
        const hasData = present > 0
        const allZero = hasData && states.every((s) => s === 'none' || s === 'zero')
        const open = openOverride[g.key] ?? hasData
        const allChecked = g.ids.every((id) => checked.has(id))
        const someChecked = g.ids.some((id) => checked.has(id))
        const note = !hasData
          ? g.key === 'mag'
            ? magHint(hint, scopeStore.stateOf('mag.raw_total', nowT) !== 'none')
            : g.emptyHint(hint)
          : allZero
            ? 'All values are 0 — motors idle, or this ESC reports no telemetry.'
            : null
        return (
          <div className="signal-group" key={g.key}>
            <div
              className="signal-group-head"
              onClick={() => setOpenOverride((o) => ({ ...o, [g.key]: !open }))}
            >
              <input
                type="checkbox"
                checked={allChecked}
                ref={(el) => {
                  if (el) el.indeterminate = someChecked && !allChecked
                }}
                title="Plot / hide every signal in this group"
                onClick={(e) => e.stopPropagation()}
                onChange={() => onToggle(g.ids, !allChecked)}
              />
              <span className={open ? 'caret' : 'caret caret-closed'} />
              <span className="group-title">{g.title}</span>
              <span className="msg">
                {present}/{g.ids.length}
              </span>
            </div>
            {note && <div className="signal-hint">{note}</div>}
            {open &&
              g.ids.map((id) => (
                <SignalRow
                  key={id}
                  id={id}
                  checked={checked.has(id)}
                  state={scopeStore.stateOf(id, nowT)}
                  onToggle={() => onToggle([id], !checked.has(id))}
                />
              ))}
          </div>
        )
      })}
      {extra.length > 0 && (
        <div className="signal-group">
          <div className="signal-group-head">
            <span className="group-title">Other ESCs</span>
            <span className="msg">{extra.length}</span>
          </div>
          {extra.map((id) => (
            <SignalRow
              key={id}
              id={id}
              checked={checked.has(id)}
              state={scopeStore.stateOf(id, nowT)}
              onToggle={() => onToggle([id], !checked.has(id))}
            />
          ))}
        </div>
      )}
    </div>
  )
}

/** The magnetometer group's empty-state hint: raw arrives with the link, the
 * calibrated trace additionally needs a loaded calibration. */
function magHint(hint: HintContext, rawSeen: boolean): string | null {
  if (!hint.targetConnected && !rawSeen) return 'Connect the PX4 serial link.'
  if (!rawSeen) return 'Waiting for HIGHRES_IMU…'
  return null
}

function SignalRow({
  id,
  checked,
  state,
  onToggle,
}: {
  id: string
  checked: boolean
  state: SignalState
  onToggle: () => void
}) {
  const def = defOf(id)
  if (!def) return null
  const st = scopeStore.stats.get(id)
  const color = colorOf(def)
  const stateTitle: Record<SignalState, string> = {
    none: 'No data yet',
    zero: 'Only zeros so far',
    live: 'Receiving',
    stale: 'No sample for more than 2 s',
    released: 'No active PWM command',
  }
  return (
    <label className={`signal-row ${state === 'none' ? 'dim' : ''}`}>
      <input type="checkbox" checked={checked} onChange={onToggle} />
      <svg className="line-swatch" width="22" height="8" aria-hidden>
        <line
          x1="0"
          y1="4"
          x2="22"
          y2="4"
          stroke={color}
          strokeWidth="2"
          strokeDasharray={def.dash ? def.dash.join(' ') : undefined}
        />
      </svg>
      <span className="mono signal-name">{def.label}</span>
      <span className={`state-dot ${state}`} title={stateTitle[state]} />
      <span className="mono signal-value">
        {st && Number.isFinite(st.last) ? `${fmtValue(st.last, def)} ${def.unit}` : '—'}
      </span>
    </label>
  )
}

function fmtValue(v: number, def: SignalDef): string {
  if (def.unit === 'nT') return v.toFixed(1)
  if (def.unit === 'V' || def.unit === 'A') return v.toFixed(2)
  return v.toFixed(0)
}

// ---------------------------------------------------------------- pane ---

interface PaneProps {
  title: string
  defs: SignalDef[]
  windowRef: MutableRefObject<number>
  /** Frozen reference time while paused (host epoch s), else null. */
  pausedAtRef: MutableRefObject<number | null>
}

function ScopePane({ title, defs, windowRef, pausedAtRef }: PaneProps) {
  const hostRef = useRef<HTMLDivElement | null>(null)
  // The plot is rebuilt only when the set of series changes.
  const signature = defs.map((d) => d.id).join('|')

  useEffect(() => {
    const host = hostRef.current
    if (!host) return
    const units = [...new Set(defs.map((d) => d.unit))]
    const muted = cssVar('--mg-muted')
    const border = cssVar('--mg-border')
    const runColor = withAlpha(cssVar('--mg-warn'), 0.12)
    // x values are seconds relative to the reference time (≤ 0).
    let tRef = Date.now() / 1000

    const xValues = (_u: uPlot, vals: number[]) => vals.map((v) => `${v.toFixed(0)} s`)
    const opts: uPlot.Options = {
      width: Math.max(200, host.clientWidth),
      height: Math.max(100, host.clientHeight),
      legend: { show: true, live: true },
      cursor: { sync: { key: SYNC_KEY } },
      scales: {
        x: {
          time: false,
          auto: false,
          range: () => [-windowRef.current, 0] as [number, number],
        },
        ...Object.fromEntries(units.map((u) => [u, { auto: true }])),
      },
      axes: [
        { stroke: muted, grid: { stroke: border }, values: xValues },
        ...units.map((u, i) => ({
          scale: u,
          side: (i === 0 ? 3 : 1) as 1 | 3,
          label: u,
          stroke: muted,
          grid: i === 0 ? { stroke: border } : { show: false },
        })),
      ],
      series: [
        { label: 't' },
        ...defs.map((d) => ({
          label: d.label,
          scale: d.unit,
          stroke: colorOf(d),
          width: 1.5,
          dash: d.dash ?? undefined,
          spanGaps: !d.id.startsWith('cmd.pwm'),
          points: { show: false },
        })),
      ],
      plugins: [
        {
          hooks: {
            // Shade the intervals where the motor test was sending, under
            // the curves: makes "before vs during" obvious at a glance.
            drawClear: (u: uPlot) => {
              const { left, top, width, height } = u.bbox
              const ctx = u.ctx
              ctx.save()
              ctx.fillStyle = runColor
              for (const r of scopeStore.runs) {
                const x0 = u.valToPos(r.t0 - tRef, 'x', true)
                const x1 = r.t1 === null ? left + width : u.valToPos(r.t1 - tRef, 'x', true)
                const a = Math.max(left, Math.min(x0, x1))
                const b = Math.min(left + width, Math.max(x0, x1))
                if (b > a) ctx.fillRect(a, top, b - a, height)
              }
              ctx.restore()
            },
          },
        },
      ],
    }
    const plot = new uPlot(opts, emptyData(defs.length, windowRef.current), host)

    const fit = () => {
      const legend = plot.root.querySelector<HTMLElement>('.u-legend')
      const legendH = legend ? legend.offsetHeight : 0
      plot.setSize({
        width: Math.max(200, host.clientWidth),
        height: Math.max(80, host.clientHeight - legendH - 6),
      })
    }
    fit()
    const ro = new ResizeObserver(fit)
    ro.observe(host)

    const timer = window.setInterval(() => {
      tRef = pausedAtRef.current ?? Date.now() / 1000
      plot.setData(buildData(defs, tRef, windowRef.current), true)
    }, REDRAW_MS)

    return () => {
      window.clearInterval(timer)
      ro.disconnect()
      plot.destroy()
    }
    // `defs` is covered by `signature`; the refs are stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature])

  return (
    <div className="scope-pane">
      <div className="scope-pane-title">{title}</div>
      <div ref={hostRef} className="scope-pane-plot" />
    </div>
  )
}

function emptyData(seriesCount: number, windowS: number): uPlot.AlignedData {
  return [
    new Float64Array([-windowS, 0]),
    ...Array.from({ length: seriesCount }, () => [null, null] as (number | null)[]),
  ] as unknown as uPlot.AlignedData
}

/** Merge the pane's rings onto one sorted union time axis (uPlot needs a
 * shared x). Undefined marks a different signal's timestamp; explicit null
 * breaks a released PWM command, so interleaved telemetry does not erase
 * the distinction between sparse sampling and a real release. */
function buildData(defs: SignalDef[], tRef: number, windowS: number): uPlot.AlignedData {
  const cutoff = tRef - windowS
  const slices = defs.map((d) => {
    const ring = scopeStore.rings.get(d.id)
    return ring ? { ring, i0: ring.firstAtOrAfter(cutoff) } : null
  })

  const all: number[] = []
  for (const s of slices) {
    if (!s) continue
    for (let i = s.i0; i < s.ring.length; i++) {
      const t = s.ring.timeAt(i)
      if (t > tRef) break
      all.push(t)
    }
  }
  if (all.length < 2) return emptyData(defs.length, windowS)
  all.sort((a, b) => a - b)
  const xsAbs: number[] = []
  for (const t of all) {
    if (xsAbs.length === 0 || t !== xsAbs[xsAbs.length - 1]) xsAbs.push(t)
  }

  const cols = slices.map((s) => {
    const col: (number | null | undefined)[] = new Array(xsAbs.length).fill(undefined)
    if (!s) return col
    for (let i = s.i0; i < s.ring.length; i++) {
      const t = s.ring.timeAt(i)
      if (t > tRef) break
      const value = s.ring.valueAt(i)
      col[lowerBound(xsAbs, t)] = Number.isFinite(value) ? value : null
    }
    return col
  })
  const xs = Float64Array.from(xsAbs, (t) => t - tRef)
  return [xs, ...cols] as unknown as uPlot.AlignedData
}

function lowerBound(xs: number[], t: number): number {
  let lo = 0
  let hi = xs.length
  while (lo < hi) {
    const mid = (lo + hi) >> 1
    if (xs[mid] < t) lo = mid + 1
    else hi = mid
  }
  return lo
}
