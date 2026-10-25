// Signal scope for the motor-test window. Layout follows the Signal
// Inspector: a signal list with checkboxes on the left, plots on the right.
//
//   - SITL servo 1–4 (µs) and mapped motor outputs from the serial target
//   - converted ACTUATOR_TEST output (µs or DShot counts, with configuration metadata)
//   - total field before / after calibration (nT)
//   - total battery current from BATTERY_STATUS (A)
//   - ESC rpm / current / voltage — listed as soon as ESC_STATUS arrives
//
// Top and bottom panes have separate signal selections. A signal may appear
// in either or both panes; different units keep separate axes. Both share a
// rolling time window and a cursor. Colour = motor number, dash = source or
// quantity, so SITL vs serial for the same motor line up.

import { useCallback, useEffect, useMemo, useRef, useState, type MutableRefObject, type ReactNode } from 'react'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'
import { listen } from '@tauri-apps/api/event'
import { cssVar } from '../design-system/theme'
import { isTauri } from '../inspector/mock'
import {
  LIST_GROUPS,
  defOf,
  normalizeTraceColor,
  scopeStore,
  type HintContext,
  type IncomingSample,
  type ScopeGroup,
  type SignalDef,
  type SignalState,
  type ScopeOutputMeta,
} from './scopeStore'

/** Plot redraw cadence; the shell flushes batches at ~20 Hz. */
const REDRAW_MS = 100
/** List refresh cadence (status dots, last values). */
const LIST_MS = 500
const WINDOWS_S = [10, 30, 60, 120]
const DEFAULT_WINDOW_S = 30
const MIN_LIST_WIDTH = 220
const MAX_LIST_WIDTH = 560
const DEFAULT_LIST_WIDTH = 300
const LIST_STEP = 16
const RESIZER_WIDTH = 8
const MIN_PLOT_WIDTH = 240
const MIN_PLOT_HEIGHT = 80
const TIME_AXIS_HEIGHT = 38
const PLOT_FRAME_GAP = 6
const clampListWidth = (width: number) =>
  Math.round(Math.min(MAX_LIST_WIDTH, Math.max(MIN_LIST_WIDTH, width)))
const STORE_KEY = 'maggcs.actuator.scope'
const SYNC_KEY = 'maggcs-motor-scope'

type PaneId = 'top' | 'bottom'

interface PaneSelection {
  checked: string[]
  /** First-detection choices and manual changes are remembered per pane. */
  seen: string[]
}

type PaneSelections = Record<PaneId, PaneSelection>

const PANE_IDS: PaneId[] = ['top', 'bottom']
const PANE_LABELS: Record<PaneId, string> = { top: 'Top', bottom: 'Bottom' }
const DEFAULT_GROUPS: Record<PaneId, ScopeGroup> = { top: 'cmd', bottom: 'mag' }

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
  const outputs: ScopeOutputMeta[] = [1, 2, 3, 4].map((motor) => ({
    connection_id: 0,
    revision: 1,
    motor,
    channel: { port: 'main', channel: motor },
    unit: 'microseconds',
  }))
  scopeStore.configureOutputs(outputs)
  const t0 = Date.now() / 1000
  const id = window.setInterval(() => {
    const t = Date.now() / 1000
    const x = t - t0
    const ramp = Math.min(1, Math.max(0, Math.sin(x / 6) * 1.5))
    const running = ramp > 0.05
    const batch: IncomingSample[] = []
    for (const [m, output] of outputs.entries()) {
      const us = 1000 + 1000 * ramp * (0.85 + 0.05 * m)
      batch.push({ id: `sitl.servo${m + 1}`, t, v: us })
      batch.push({ id: `fc.motor${m + 1}`, t: t + 0.01, v: us - 4, output })
      batch.push({ id: `cmd.pwm${m + 1}`, t, v: running ? 1100 + 800 * ramp * (0.85 + 0.05 * m) : null, output })
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
  panes: PaneSelections
  activePane: PaneId
  windowS: number
  /** Width of the signal list in px; the column is drag-resizable. */
  listWidth: number
  colors: Record<string, string>
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function normalizeIds(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  return [...new Set(value
    .filter((id): id is string => typeof id === 'string')
    .map((id) => id.replace(/^fwd\.cmd([1-4])$/, 'cmd.pwm$1')))]
}

function migrateSelections(p: Record<string, unknown>): PaneSelections {
  const checked = normalizeIds(p.checked)
  const seen = [...new Set([...normalizeIds(p.seen), ...checked])]
  const valid = (value: unknown): value is ScopeGroup =>
    value === 'cmd' || value === 'mag' || value === 'esc' || value === 'bat'
  let top = valid(p.activeView) ? p.activeView : DEFAULT_GROUPS.top
  let bottom: ScopeGroup = top === 'cmd' ? 'mag' : 'cmd'
  if (Array.isArray(p.views) && p.views.length === 2 && p.views.every(valid)) {
    top = p.views[0]
    bottom = p.views[1]
  }
  // Preserve every formerly checked signal. The top view stays on top;
  // other selections go below, including groups formerly hidden by tabs.
  return {
    top: { checked: checked.filter((id) => defOf(id)?.group === top), seen: [...seen] },
    bottom: {
      checked: checked.filter((id) => defOf(id)?.group !== top || defOf(id)?.group === bottom),
      seen: [...seen],
    },
  }
}

function loadPersisted(): Persisted {
  try {
    const raw = localStorage.getItem(STORE_KEY)
    if (raw) {
      const p: unknown = JSON.parse(raw)
      if (!isRecord(p)) throw new Error('Invalid scope preferences')
      const panes = migrateSelections(p)
      if (isRecord(p.panes)) {
        for (const id of PANE_IDS) {
          const selection = p.panes[id]
          if (!isRecord(selection) || !Array.isArray(selection.checked) || !Array.isArray(selection.seen)) continue
          const checked = normalizeIds(selection.checked)
          panes[id] = {
            checked,
            seen: [...new Set([...normalizeIds(selection.seen), ...checked])],
          }
        }
      }
      const colors = Object.fromEntries(
        Object.entries(isRecord(p.colors) ? p.colors : {})
          .map(([id, color]) => [id, normalizeTraceColor(color)] as const)
          .filter((entry): entry is [string, `#${string}`] => entry[1] !== null),
      )
      return {
        panes,
        activePane: p.activePane === 'bottom' ? 'bottom' : 'top',
        windowS: typeof p.windowS === 'number' && WINDOWS_S.includes(p.windowS) ? p.windowS : DEFAULT_WINDOW_S,
        listWidth: typeof p.listWidth === 'number' && Number.isFinite(p.listWidth)
          ? clampListWidth(p.listWidth)
          : DEFAULT_LIST_WIDTH,
        colors,
      }
    }
  } catch {
    /* corrupt or unavailable storage: start fresh */
  }
  return {
    panes: { top: { checked: [], seen: [] }, bottom: { checked: [], seen: [] } },
    activePane: 'top',
    windowS: DEFAULT_WINDOW_S,
    listWidth: DEFAULT_LIST_WIDTH,
    colors: {},
  }
}

function savePersisted(p: Persisted): void {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(p))
  } catch {
    /* storage full / unavailable: the scope still works */
  }
}

// -------------------------------------------------------------- colour ---

function defaultColorOf(def: SignalDef): string {
  if (def.motor !== null) return cssVar(`--mg-trace-${def.motor + 1}`)
  return def.id === 'mag.cal_total' ? cssVar('--mg-trace-total-mag') : cssVar('--mg-trace-5')
}

function colorOf(def: SignalDef, colors: Record<string, string>): string {
  return colors[def.id] ?? defaultColorOf(def)
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
  tools,
}: HintContext & { legendOpen?: boolean; onToggleLegend?: () => void; tools?: ReactNode }) {
  const initial = useMemo(loadPersisted, [])
  const [panes, setPanes] = useState<PaneSelections>(initial.panes)
  const [activePane, setActivePane] = useState<PaneId>(initial.activePane)
  const [windowS, setWindowS] = useState(initial.windowS)
  const [listWidth, setListWidth] = useState(initial.listWidth)
  const [colors, setColors] = useState<Record<string, string>>(initial.colors)
  const [paused, setPaused] = useState(false)
  const [appearance, setAppearance] = useState(0)
  const [, setTick] = useState(0)
  const checked = useMemo(() => new Set(panes[activePane].checked), [activePane, panes])
  const pausedAtRef = useRef<number | null>(null)
  const windowRef = useRef(windowS)
  windowRef.current = windowS
  const dragRef = useRef<{ startX: number; startWidth: number } | null>(null)

  // uPlot resolves its canvas colors at construction. Rebuild only the plots
  // when the window's theme/density changes, keeping samples and selection.
  useEffect(() => {
    const root = document.documentElement
    const appearanceKey = () => `${root.dataset.theme}:${root.classList.contains('field-mode')}`
    let previous = appearanceKey()
    const observer = new MutationObserver(() => {
      const next = appearanceKey()
      if (next === previous) return
      previous = next
      setAppearance((v) => v + 1)
    })
    observer.observe(root, {
      attributes: true,
      attributeFilter: ['data-theme', 'class'],
    })
    return () => observer.disconnect()
  }, [])

  // The store outlives this component; tell it what was already offered.
  useEffect(() => {
    const bottomSeen = new Set(initial.panes.bottom.seen)
    scopeStore.preloadOffered(initial.panes.top.seen.filter((id) => bottomSeen.has(id)))
  }, [initial.panes])

  // Persist both independent selections, including deliberate unchecks.
  useEffect(() => {
    savePersisted({
      panes,
      activePane,
      windowS,
      listWidth,
      colors,
    })
  }, [activePane, colors, listWidth, panes, windowS])

  // List refresh + auto-check of newly detected signals.
  useEffect(() => {
    const id = window.setInterval(() => {
      scopeStore.advanceTime(Date.now() / 1000)
      // Default routing is output above and magnetic field below. Other
      // signals are available for either pane without altering a selection.
      const fresh = scopeStore.takeNew()
      if (fresh.length > 0) {
        setPanes((previous) => {
          const next = { ...previous }
          for (const pane of PANE_IDS) {
            const seen = new Set(previous[pane].seen)
            const selected = new Set(previous[pane].checked)
            for (const id of fresh) {
              const def = defOf(id)
              if (!seen.has(id) && def?.defaultOn && def.group === DEFAULT_GROUPS[pane]) selected.add(id)
              seen.add(id)
            }
            next[pane] = { checked: [...selected], seen: [...seen] }
          }
          return next
        })
      }
      setTick((n) => n + 1)
    }, LIST_MS)
    return () => window.clearInterval(id)
  }, [])

  const toggleSignals = useCallback((pane: PaneId, ids: string[], enabled: boolean) => {
    setPanes((previous) => {
      const selected = new Set(previous[pane].checked)
      const seen = new Set(previous[pane].seen)
      for (const id of ids) {
        if (enabled) selected.add(id)
        else selected.delete(id)
        seen.add(id)
      }
      return { ...previous, [pane]: { checked: [...selected], seen: [...seen] } }
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

  const setTraceColor = useCallback((id: string, color: string) => {
    const normalized = normalizeTraceColor(color)
    if (!normalized) return
    setColors((previous) => ({ ...previous, [id]: normalized }))
  }, [])

  const selectPane = (pane: PaneId) => {
    setActivePane(pane)
    if (!legendOpen) onToggleLegend?.()
  }

  useEffect(() => () => document.documentElement.classList.remove('scope-resizing'), [])

  const onResizeStart = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!legendOpen || event.button !== 0) return
    event.preventDefault()
    dragRef.current = { startX: event.clientX, startWidth: listWidth }
    event.currentTarget.setPointerCapture(event.pointerId)
    document.documentElement.classList.add('scope-resizing')
  }
  const onResizeMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current
    if (!drag) return
    setListWidth(clampListWidth(drag.startWidth + event.clientX - drag.startX))
  }
  const onResizeEnd = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!dragRef.current) return
    dragRef.current = null
    document.documentElement.classList.remove('scope-resizing')
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
  }
  const onResizeLostCapture = () => {
    dragRef.current = null
    document.documentElement.classList.remove('scope-resizing')
  }
  const onResizeKey = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
      event.preventDefault()
      setListWidth((width) => clampListWidth(width + (event.key === 'ArrowLeft' ? -LIST_STEP : LIST_STEP)))
    } else if (event.key === 'Home') {
      event.preventDefault()
      setListWidth(MIN_LIST_WIDTH)
    } else if (event.key === 'End') {
      event.preventDefault()
      setListWidth(MAX_LIST_WIDTH)
    }
  }

  const hint: HintContext = { sitlConnected, targetConnected, calLoaded }
  const nowT = Date.now() / 1000

  const paneDefs = (pane: PaneId): SignalDef[] => {
    const out: SignalDef[] = []
    const seen = new Set<string>()
    const selected = new Set(panes[pane].checked)
    for (const g of LIST_GROUPS) {
      for (const id of g.ids) {
        const d = defOf(id)
        if (d && selected.has(id) && !seen.has(id)) {
          out.push(d)
          seen.add(id)
        }
      }
    }
    // ESC 5–8 and other dynamic ids that are checked but not in the list.
    for (const id of selected) {
      const d = defOf(id)
      if (d && !seen.has(id)) {
        out.push(d)
        seen.add(id)
      }
    }
    return out
  }
  return (
    <div
      className={`scope-grid${legendOpen ? ' legend-open' : ''}`}
      style={legendOpen ? {
        gridTemplateColumns: `minmax(${MIN_LIST_WIDTH}px, min(${listWidth}px, calc(100% - ${MIN_PLOT_WIDTH + RESIZER_WIDTH}px))) ${RESIZER_WIDTH}px minmax(0, 1fr)`,
      } : undefined}
    >
      <SignalList
        checked={checked}
        colors={colors}
        hint={hint}
        nowT={nowT}
        activePane={activePane}
        onSelectPane={setActivePane}
        onColor={setTraceColor}
        onToggle={(ids, enabled) => toggleSignals(activePane, ids, enabled)}
      />
      {legendOpen && (
        <div
          className="scope-resizer"
          role="separator"
          aria-label="Resize signal list"
          aria-orientation="vertical"
          aria-valuemin={MIN_LIST_WIDTH}
          aria-valuemax={MAX_LIST_WIDTH}
          aria-valuenow={listWidth}
          tabIndex={0}
          title="Drag to resize the signal list; double-click to reset."
          onPointerDown={onResizeStart}
          onPointerMove={onResizeMove}
          onPointerUp={onResizeEnd}
          onPointerCancel={onResizeEnd}
          onLostPointerCapture={onResizeLostCapture}
          onDoubleClick={() => setListWidth(DEFAULT_LIST_WIDTH)}
          onKeyDown={onResizeKey}
        />
      )}
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
          {tools}
          <span className="scope-legend-note">
            <span className="run-swatch" /> motor test sending
          </span>
        </div>
        <div className="scope-panes">
          {PANE_IDS.map((pane) => {
            const defs = paneDefs(pane)
            const label = PANE_LABELS[pane]
            return (
              <section className="scope-pane" data-pane={pane} key={pane} aria-label={`${label} plot`}>
                <div className="scope-pane-head">
                  <span className="scope-pane-title">{label} plot</span>
                  <span className="scope-pane-count mono">{defs.length} signals</span>
                  <button
                    className={`mode-btn scope-btn${activePane === pane && legendOpen ? ' active' : ''}`}
                    aria-label={`Select signals for ${label.toLowerCase()} plot`}
                    aria-pressed={activePane === pane && legendOpen}
                    aria-controls="motor-signal-list"
                    onClick={() => selectPane(pane)}
                    title={`Choose signals for the ${label.toLowerCase()} plot`}
                  >
                    Signals
                  </button>
                </div>
                {defs.length === 0 ? (
                  <div className="scope-empty">
                    Choose Signals to add traces to the {label.toLowerCase()} plot.
                  </div>
                ) : (
                  <ScopePane
                    key={`${pane}:${appearance}`}
                    colors={colors}
                    defs={defs}
                    windowRef={windowRef}
                    pausedAtRef={pausedAtRef}
                  />
                )}
              </section>
            )
          })}
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- list ---

function SignalList({
  checked,
  colors,
  hint,
  nowT,
  activePane,
  onSelectPane,
  onColor,
  onToggle,
}: {
  checked: Set<string>
  colors: Record<string, string>
  hint: HintContext
  nowT: number
  activePane: PaneId
  onSelectPane: (pane: PaneId) => void
  onColor: (id: string, color: string) => void
  onToggle: (ids: string[], on: boolean) => void
}) {
  /** Operator overrides of a group's open/closed state; absent = open when
   * the group has data (groups without data start collapsed). */
  const [openOverride, setOpenOverride] = useState<Record<string, boolean>>({})
  const [query, setQuery] = useState('')
  const normalizedQuery = query.trim().toLowerCase()
  const shown = (id: string) => {
    if (!normalizedQuery) return true
    const def = defOf(id)
    return id.toLowerCase().includes(normalizedQuery)
      || def?.label.toLowerCase().includes(normalizedQuery) === true
  }

  // ESC 5–8 (and other dynamic ids) that have data but are not in the static
  // groups get their own section so a larger airframe is still reachable.
  const extra = [...new Set([...scopeStore.stats.keys(), ...checked])].filter(
    (id) => defOf(id) && shown(id) && !LIST_GROUPS.some((g) => g.ids.includes(id)),
  )
  extra.sort((a, b) => a.localeCompare(b, undefined, { numeric: true }))

  return (
    <div className="signal-list" id="motor-signal-list">
      <div className="signal-list-heading">
        <span className="signal-list-title">Signals</span>
        <div className="signal-pane-switch" role="group" aria-label="Choose plot for signal selection">
          {PANE_IDS.map((pane) => (
            <button
              key={pane}
              className={`mode-btn scope-btn${activePane === pane ? ' active' : ''}`}
              aria-label={`Edit ${pane} plot signals`}
              aria-pressed={activePane === pane}
              onClick={() => onSelectPane(pane)}
            >
              {PANE_LABELS[pane]}
            </button>
          ))}
        </div>
      </div>
      <div className="signal-pane-note">Selecting for the {activePane} plot</div>
      <input
        className="signal-search"
        type="search"
        value={query}
        placeholder="Search signals"
        aria-label="Search signals"
        onChange={(event) => setQuery(event.target.value)}
      />
      {LIST_GROUPS.map((g) => {
        const visibleIds = g.ids.filter(shown)
        if (visibleIds.length === 0) return null
        const states = visibleIds.map((id) => scopeStore.stateOf(id, nowT))
        const present = states.filter((s) => s !== 'none').length
        const hasData = present > 0
        const allZero = hasData && states.every((s) => s === 'none' || s === 'zero')
        const allChecked = visibleIds.every((id) => checked.has(id))
        const someChecked = visibleIds.some((id) => checked.has(id))
        const open = normalizedQuery !== '' || (openOverride[g.key] ?? (hasData || someChecked))
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
                aria-label={`Plot ${g.title} in ${activePane} plot`}
                title={`Plot / hide this group in the ${activePane} plot`}
                onClick={(e) => e.stopPropagation()}
                onChange={() => onToggle(visibleIds, !allChecked)}
              />
              <span className={open ? 'caret' : 'caret caret-closed'} />
              <span className="group-title" title={g.title}>{g.title}</span>
              <span className="msg">
                  {present}/{visibleIds.length}
              </span>
            </div>
            {note && <div className="signal-hint">{note}</div>}
            {open &&
              visibleIds.map((id) => (
                <SignalRow
                  key={id}
                  id={id}
                  pane={activePane}
                  checked={checked.has(id)}
                  color={colors[id]}
                  state={scopeStore.stateOf(id, nowT)}
                  onColor={(color) => onColor(id, color)}
                  onToggle={() => onToggle([id], !checked.has(id))}
                />
              ))}
          </div>
        )
      })}
      {extra.length > 0 && (
        <div className="signal-group">
          <div className="signal-group-head">
            <span className="group-title">Other signals</span>
            <span className="msg">{extra.length}</span>
          </div>
          {extra.map((id) => (
            <SignalRow
              key={id}
              id={id}
              pane={activePane}
              checked={checked.has(id)}
              color={colors[id]}
              state={scopeStore.stateOf(id, nowT)}
              onColor={(color) => onColor(id, color)}
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
  pane,
  checked,
  color,
  state,
  onColor,
  onToggle,
}: {
  id: string
  pane: PaneId
  checked: boolean
  color: string | undefined
  state: SignalState
  onColor: (color: string) => void
  onToggle: () => void
}) {
  const def = defOf(id)
  if (!def) return null
  const st = scopeStore.stats.get(id)
  const stroke = color ?? defaultColorOf(def)
  const pickerColor = normalizeTraceColor(stroke) ?? ''
  const stateTitle: Record<SignalState, string> = {
    none: 'No data yet',
    zero: 'Only zeros so far',
    live: 'Receiving',
    stale: 'No sample for more than 2 s',
    released: 'No active PWM command',
    boundary: 'Run ended; the next run starts a new segment',
  }
  return (
    <div className={`signal-row ${state === 'none' ? 'dim' : ''}`}>
      <input
        type="checkbox"
        checked={checked}
        aria-label={`Plot ${def.label} in ${pane} plot`}
        onChange={onToggle}
      />
      <svg className="line-swatch" width="22" height="8" aria-hidden>
        <line
          x1="0"
          y1="4"
          x2="22"
          y2="4"
          stroke={stroke}
          strokeWidth="2"
          strokeDasharray={def.dash ? def.dash.join(' ') : undefined}
        />
      </svg>
      <span className="mono signal-name" title={def.label}>{def.label}</span>
      <span className={`state-dot ${state}`} title={stateTitle[state]} />
      <span className="mono signal-value">
        {st && state !== 'released' && state !== 'boundary' && Number.isFinite(st.last)
          ? `${fmtValue(st.last, def)} ${def.unit}`
          : '—'}
      </span>
      <input
        className="signal-color"
        type="color"
        value={pickerColor}
        aria-label={`Color for ${def.label}`}
        title={`Color for ${def.label}`}
        onChange={(event) => onColor(event.target.value)}
      />
    </div>
  )
}

function fmtValue(v: number, def: SignalDef): string {
  if (def.unit === 'nT') return v.toFixed(1)
  if (def.unit === 'V' || def.unit === 'A') return v.toFixed(2)
  return v.toFixed(0)
}

// ---------------------------------------------------------------- pane ---

interface PaneProps {
  colors: Record<string, string>
  defs: SignalDef[]
  windowRef: MutableRefObject<number>
  /** Frozen reference time while paused (host epoch s), else null. */
  pausedAtRef: MutableRefObject<number | null>
}

function ScopePane({ colors, defs, windowRef, pausedAtRef }: PaneProps) {
  const hostRef = useRef<HTMLDivElement | null>(null)
  // Units, labels and configuration identity are part of the mounted plot.
  const signature = JSON.stringify(defs.map((d) => [
    d.id, d.label, d.unit, d.contextKey, d.motor, d.dash, d.staleAfterS, colors[d.id],
  ]))

  useEffect(() => {
    const host = hostRef.current
    if (!host) return
    const units = [...new Set(defs.map((d) => d.unit))]
    const muted = cssVar('--mg-muted')
    const border = cssVar('--mg-border')
    const runColor = withAlpha(cssVar('--mg-warn'), 0.12)
    // tRef remains epoch time for stale/window filtering; plotted x values are
    // cumulative seconds from the first accepted value.
    let tRef = Date.now() / 1000

    const xValues = (_u: uPlot, vals: number[]) => vals.map((v) => `${v.toFixed(0)} s`)
    const opts: uPlot.Options = {
      width: Math.max(MIN_PLOT_WIDTH, host.clientWidth),
      height: Math.max(MIN_PLOT_HEIGHT, host.clientHeight),
      legend: { show: true, live: true },
      cursor: { sync: { key: SYNC_KEY } },
      scales: {
        x: {
          time: false,
          auto: false,
          range: () => elapsedWindow(tRef, windowRef.current),
        },
        ...Object.fromEntries(units.map((u) => [u, { auto: true }])),
      },
      axes: [
        { stroke: muted, grid: { stroke: border }, values: xValues, size: TIME_AXIS_HEIGHT },
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
          stroke: colorOf(d, colors),
          width: 1.5,
          dash: d.dash ?? undefined,
          spanGaps: false,
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
                const x0 = u.valToPos(scopeStore.elapsedAt(r.t0), 'x', true)
                const x1 = r.t1 === null
                  ? left + width
                  : u.valToPos(scopeStore.elapsedAt(r.t1), 'x', true)
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
    const plot = new uPlot(opts, emptyData(defs.length, tRef, windowRef.current), host)
    if (import.meta.env.DEV) {
      Object.defineProperty(host, '__maggcsScopePlot', {
        configurable: true,
        value: plot,
      })
    }

    const legend = plot.root.querySelector<HTMLElement>('.u-legend')
    const fit = () => {
      const width = Math.max(MIN_PLOT_WIDTH, host.clientWidth)
      // Set width first so legend wrapping is measured at the new width.
      if (plot.width !== width) plot.setSize({ width, height: plot.height })
      const legendH = legend ? legend.offsetHeight : 0
      const height = Math.max(MIN_PLOT_HEIGHT, host.clientHeight - legendH - PLOT_FRAME_GAP)
      if (plot.height !== height) plot.setSize({ width, height })
    }
    fit()
    const ro = new ResizeObserver(fit)
    ro.observe(host)
    if (legend) ro.observe(legend)

    const timer = window.setInterval(() => {
      tRef = pausedAtRef.current ?? Date.now() / 1000
      if (pausedAtRef.current === null) scopeStore.advanceTime(tRef)
      plot.setData(buildData(defs, tRef, windowRef.current), true)
    }, REDRAW_MS)

    return () => {
      window.clearInterval(timer)
      ro.disconnect()
      if (import.meta.env.DEV) delete (host as HTMLDivElement & { __maggcsScopePlot?: uPlot }).__maggcsScopePlot
      plot.destroy()
    }
    // `defs` is covered by `signature`; the refs are stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature])

  return <div ref={hostRef} className="scope-pane-plot" />
}

function elapsedWindow(tRef: number, windowS: number): [number, number] {
  const elapsed = scopeStore.elapsedAt(tRef)
  const end = Math.max(windowS, elapsed)
  return [Math.max(0, end - windowS), end]
}

function emptyData(seriesCount: number, tRef: number, windowS: number): uPlot.AlignedData {
  const [start, end] = elapsedWindow(tRef, windowS)
  return [
    new Float64Array([start, end]),
    ...Array.from({ length: seriesCount }, () => [null, null] as (number | null)[]),
  ] as unknown as uPlot.AlignedData
}

/** Merge the pane's rings onto one sorted union time axis (uPlot needs a
 * shared x). Undefined marks another signal's timestamp; explicit null is a
 * release, stale or run boundary and breaks the affected series. */
function buildData(defs: SignalDef[], tRef: number, windowS: number): uPlot.AlignedData {
  const cutoff = tRef - windowS
  const slices = defs.map((d) => {
    // A plot awaiting its rebuild must not read another configuration's ring.
    if ((scopeStore.definitionOf(d.id)?.contextKey ?? '') !== (d.contextKey ?? '')) return null
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
  if (all.length < 2) return emptyData(defs.length, tRef, windowS)
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
  const xs = Float64Array.from(xsAbs, (t) => scopeStore.elapsedAt(t))
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
