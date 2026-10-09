// Signal Inspector (plan §6, ADR-016): a separate window that buffers the raw
// signal stream into ring buffers and renders uPlot time plots with raw +
// filtered traces. In Tauri mode the filter pipeline runs in Rust
// (`core::inspector::session`, ADR-015) and the frames carry the filtered
// values; in browser mock mode a TS mirror of `core::dsp` previews the filters.
// P7: run/pause, window duration, clear, cursor, CSV export, axis linking, a
// properties panelainer, workspace save/loadasiato and SET_MESSAGE_INTERVAL rate control.

import { useCallback, useEffect, useRef, useState } from 'react'
import uPlot from 'uplot'
import { invoke, Channel } from '@tauri-apps/api/core'
import { makeProcessor, magnitudeSpectrum, type AlgorithmInfo } from './dsp'
import { isTauri, mockCatalog, mockSamples, signalKey } from './mock'
import type { CatalogEntry } from '../generated-types/CatalogEntry'
import type { SignalId } from '../generated-types/SignalId'
import type { TraceSample } from '../generated-types/TraceSample'

interface FramePayload {
  seq: number
  samples: TraceSample[]
}

interface FilterCfg {
  algo: string
  params: Record<string, number>
}

interface Buffer {
  t: number[]
  raw: number[]
  filtered: number[]
}

interface Workspace {
  windowSec: number
  checked: string[]
  filterBy: Record<string, FilterCfg>
  axisLink: boolean
}

const WS_KEY = 'maggcs.inspector.workspace'
const SYNC_KEY = 'maggcs-inspector'

function token(name: string): string {
  const cached = cssVarCache.get(name)
  if (cached) return cached
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim() || 'cyan'
  cssVarCache.set(name, v)
  return v
}
const cssVarCache = new Map<string, string>()
const ACCENT = () => token('--mg-accent')
const MUTED = () => token('--mg-muted')

/** Resolve a signal-key to its wire SignalId (fallback for a not-yet-seen signal). */
function signalFor(key: string, catalog: CatalogEntry[]): SignalId {
  const entry = catalog.find((c) => signalKey(c.signal) === key)
  return entry
    ? entry.signal
    : { system_id: 1, component_id: 1, message_id: 30, field: key.split('.').pop() ?? key }
}

export function InspectorApp() {
  const tauri = isTauri()
  const [catalog, setCatalog] = useState<CatalogEntry[]>([])
  const [algorithms, setAlgorithms] = useState<AlgorithmInfo[]>([])
  const [checked, setChecked] = useState<Set<string>>(new Set())
  const [filterBy, setFilterBy] = useState<Record<string, FilterCfg>>({})
  const [paused, setPaused] = useState(false)
  const [windowSec, setWindowSec] = useState(30)
  const [version, setVersion] = useState(0)
  const [spectrumFor, setSpectrumFor] = useState<string | null>(null)
  const [selected, setSelected] = useState<string | null>(null)
  const [axisLink, setAxisLink] = useState(false)

  const buffers = useRef<Map<string, Buffer>>(new Map())
  const plots = useRef<Map<string, uPlot>>(new Map())
  const plotHosts = useRef<Map<string, HTMLDivElement>>(new Map())

  const bump = useCallback(() => setVersion((v) => v + 1), [])

  // Workspace save/load (P7): the checked set, filters, window and axis link.
  const saveWorkspace = useCallback(() => {
    const ws: Workspace = { windowSec, checked: [...checked], filterBy, axisLink }
    localStorage.setItem(WS_KEY, JSON.stringify(ws))
  }, [windowSec, checked, filterBy, axisLink])

  const loadWorkspace = useCallback(() => {
    const raw = localStorage.getItem(WS_KEY)
    if (!raw) return
    try {
      const ws = JSON.parse(raw) as Workspace
      if (typeof ws.windowSec === 'number') setWindowSec(ws.windowSec)
      if (Array.isArray(ws.checked)) setChecked(new Set(ws.checked))
      if (ws.filterBy && typeof ws.filterBy === 'object') setFilterBy(ws.filterBy)
      setAxisLink(Boolean(ws.axisLink))
    } catch {
      // Corrupt workspace: keep the defaults.
    }
  }, [])

  // Data source: the Tauri channel when running in the app, the mock otherwise.
  useEffect(() => {
    loadWorkspace()
    if (!tauri) {
      const id = setInterval(() => {
        if (paused) return
        const now = performance.now() / 1000
        for (const s of mockSamples(now)) {
          const key = signalKey(s.id)
          const b = buffers.current.get(key) ?? { t: [], raw: [], filtered: [] }
          b.t.push(s.t_ms)
          b.raw.push(s.value)
          b.filtered.push(NaN)
          buffers.current.set(key, b)
        }
        bump()
      }, 10)
      setCatalog(mockCatalog())
      return () => clearInterval(id)
    }
    let disposed = false
    const channel = new Channel<FramePayload>()
    const onMessage = (frame: FramePayload) => {
      if (paused) return
      for (const s of frame.samples) {
        const key = signalKey(s.id)
        const b = buffers.current.get(key) ?? { t: [], raw: [], filtered: [] }
        b.t.push(s.t_ms)
        b.raw.push(s.raw)
        b.filtered.push(s.filtered)
        buffers.current.set(key, b)
      }
      bump()
    }
    channel.onmessage = onMessage
    void (async () => {
      try {
        await invoke('inspector_connect', { channel })
        const cat = await invoke<CatalogEntry[]>('inspector_catalog')
        if (!disposed) setCatalog(cat)
        const algos = await invoke<AlgorithmInfo[]>('inspector_list_algorithms')
        if (!disposed) setAlgorithms(algos)
      } catch {
        // No link: leave the tree empty; the user can still mock in the browser.
      }
    })()
    return () => {
      disposed = true
      void invoke('inspector_disconnect')
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tauri, paused, bump])

  // Push the trace set (signals + their pipelines) to the Rust session. The
  // session resets the filter for a changed trace, so clear its filtered
  // buffer: the filtered line restarts at the next sample (SDI behavior).
  useEffect(() => {
    if (!tauri) return
    const traces: Array<{ signal: SignalId; pipeline: { algorithm: string; params: [string, number][] }[]; analyzer: null }> = []
    for (const key of checked) {
      const cfg = filterBy[key]
      const pipeline =
        cfg && cfg.algo !== 'none'
          ? [{ algorithm: cfg.algo, params: Object.entries(cfg.params) as [string, number][] }]
          : []
      traces.push({ signal: signalFor(key, catalog), pipeline, analyzer: null })
    }
    void invoke('inspector_set_traces', { traces }).catch(() => undefined)
  }, [checked, filterBy, tauri, catalog])

  // On a filter change, drop the stale filtered history so the trace restarts.
  useEffect(() => {
    for (const b of buffers.current.values()) b.filtered = []
  }, [filterBy, tauri])

  // Refresh the catalog periodically so the tree fills as signals arrive.
  useEffect(() => {
    if (!tauri) return
    const id = setInterval(async () => {
      try {
        setCatalog(await invoke<CatalogEntry[]>('inspector_catalog'))
      } catch {
        /* not connected */
      }
    }, 1000)
    return () => clearInterval(id)
  }, [tauri])

  // Trim every buffer to the selected window (keep at least one FFT window).
  useEffect(() => {
    const keep = Math.max(1024, Math.ceil((windowSec * 1000) / 10))
    for (const [key, b] of buffers.current) {
      if (b.t.length > keep) {
        b.t = b.t.slice(-keep)
        b.raw = b.raw.slice(-keep)
        b.filtered = b.filtered.slice(-keep)
        buffers.current.set(key, b)
      }
    }
  }, [windowSec])

  // Create/destroy the checked plots. Rebuilt when axis linking toggles so the
  // cursor sync key applies (uPlot has no runtime setter for it).
  useEffect(() => {
    for (const key of checked) {
      if (plots.current.has(key)) continue
      const host = plotHosts.current.get(key)
      if (!host) continue
      const cursor = axisLink ? { show: true, sync: { key: SYNC_KEY } } : { show: true }
      const plot = new uPlot(
        {
          width: host.clientWidth,
          height: 180,
          legend: { show: true },
          cursor,
          axes: [
            { stroke: MUTED(), grid: { stroke: MUTED(), width: 1, dash: [2, 4] } },
            { stroke: MUTED() },
          ],
          series: [
            { label: 't', stroke: 'transparent' },
            { label: 'raw', stroke: ACCENT() },
            { label: 'filtered', stroke: token('--mg-ok'), width: 2 },
          ],
        },
        [[], [], []],
        host,
      )
      plots.current.set(key, plot)
    }
    for (const key of [...plots.current.keys()]) {
      if (!checked.has(key)) {
        plots.current.get(key)?.destroy()
        plots.current.delete(key)
      }
    }
  }, [checked, axisLink])

  // Feed data into the plots whenever new samples arrive or filters change.
  useEffect(() => {
    for (const key of checked) {
      const plot = plots.current.get(key)
      const b = buffers.current.get(key)
      if (!plot || !b || b.t.length < 2) continue
      const cfg = filterBy[key]
      const fs = estimateFs(b)
      const raw = b.raw
      const filt = tauri ? b.filtered : mirrorFilter(raw, cfg, fs)
      const data: Float64Array[] = [
        Float64Array.from(b.t),
        Float64Array.from(raw),
        Float64Array.from(filt),
      ]
      plot.setData(data as uPlot.AlignedData)
    }
  }, [version, checked, filterBy, tauri])

  useEffect(() => {
    for (const plot of plots.current.values()) plot.redraw()
  }, [windowSec])

  const toggle = (key: string) => {
    setChecked((prev) => {
      const next = new Set(prev)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }

  const clearAll = () => {
    buffers.current.clear()
    for (const p of plots.current.values()) p.setData([[], [], []])
  }

  const exportCsv = () => {
    const rows: string[] = ['t_ms,key,raw,filtered']
    for (const [key, b] of buffers.current) {
      for (let i = 0; i < b.t.length; i++) {
        rows.push(`${b.t[i].toFixed(1)},${key},${b.raw[i]},${b.filtered[i] ?? ''}`)
      }
    }
    const blob = new Blob([rows.join('\n')], { type: 'text/csv' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = 'inspector.csv'
    a.click()
    URL.revokeObjectURL(url)
  }

  const spectrum = (key: string): { bins: number[]; peakFreqHz: number; fs: number; deltaF: number } | null => {
    const b = buffers.current.get(key)
    if (!b) return null
    const n = 1024
    const recent = b.raw.slice(-n)
    if (recent.length < n) return null
    const fs = estimateFs(b)
    const s = magnitudeSpectrum(recent, fs)
    return s ? { bins: s.bins, peakFreqHz: s.peakFreqHz, fs, deltaF: s.deltaF } : null
  }

  return (
    <div className="inspector-grid">
      <div className="toolbar">
        <button className={paused ? 'active' : ''} onClick={() => setPaused(!paused)}>
          {paused ? 'Run' : 'Pause'}
        </button>
        <select value={windowSec} onChange={(e) => setWindowSec(Number(e.target.value))}>
          <option value={10}>10 s</option>
          <option value={30}>30 s</option>
          <option value={60}>60 s</option>
        </select>
        <button className={axisLink ? 'active' : ''} onClick={() => setAxisLink(!axisLink)}>
          Link axes
        </button>
        <button onClick={clearAll}>Clear data</button>
        <button onClick={exportCsv}>Export CSV</button>
        <button onClick={saveWorkspace}>Save</button>
        <button onClick={loadWorkspace}>Load</button>
        <span style={{ marginLeft: 'auto', color: 'var(--mg-muted)', fontSize: 11 }}>
          Signal Inspector{tauri ? '' : ' · mock'}
        </span>
      </div>

      <div className="signal-list">
        <div style={{ marginBottom: 6, fontWeight: 600 }}>Signals</div>
        {catalog.length === 0 && <div style={{ color: 'var(--mg-muted)' }}>Waiting for data…</div>}
        {catalog.map((c) => {
          const key = signalKey(c.signal)
          return (
            <label key={key}>
              <input type="checkbox" checked={checked.has(key)} onChange={() => toggle(key)} />
              <span className="mono">{key}</span>
              <span className="msg">
                {' '}
                {c.rate_hz.toFixed(0)} Hz · {c.last_value.toFixed(1)}
              </span>
            </label>
          )
        })}
      </div>

      <div className="plot-grid">
        {[...checked].map((key) => (
          <div className="plot" key={key}>
            <div className="plot-head">
              <span className={`title ${selected === key ? 'selected' : ''}`} onClick={() => setSelected(key)}>
                {key}
              </span>
              <select
                value={filterBy[key]?.algo ?? 'none'}
                onChange={(e) => {
                  const algo = e.target.value
                  const params: Record<string, number> = {}
                  const info = algorithms.find((a) => a.id === algo)
                  if (info) for (const p of info.params) params[p.key] = p.default
                  setFilterBy((prev) => ({ ...prev, [key]: { algo, params } }))
                }}
              >
                <option value="none">raw</option>
                {algorithms.filter((a) => a.kind === 'processor').map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name}
                  </option>
                ))}
              </select>
              {filterBy[key] && filterBy[key].algo !== 'none' && (
                <>
                  {algorithms
                    .find((a) => a.id === filterBy[key].algo)
                    ?.params.map((p) => (
                      <label key={p.key} style={{ fontSize: 11 }}>
                        {p.label}
                        <input
                          type="number"
                          style={{ width: 60, background: 'var(--mg-bg)', color: 'var(--mg-ink)', border: '1px solid var(--mg-border)', borderRadius: 4 }}
                          value={filterBy[key].params[p.key] ?? p.default}
                          onChange={(e) =>
                            setFilterBy((prev) => ({
                              ...prev,
                              [key]: { ...prev[key], params: { ...prev[key].params, [p.key]: Number(e.target.value) } },
                            }))
                          }
                        />
                      </label>
                    ))}
                </>
              )}
              <button
                className={spectrumFor === key ? 'active' : ''}
                onClick={() => setSpectrumFor(spectrumFor === key ? null : key)}
              >
                FFT
              </button>
              <button onClick={() => toggle(key)}>×</button>
            </div>
            <div ref={(el) => el && plotHosts.current.set(key, el)} className="chart" />
            {spectrumFor === key && <SpectrumView key={key} data={spectrum(key)} label={key} />}
          </div>
        ))}
      </div>

      <PropertiesPanel
        selected={selected}
        catalog={catalog}
        buffer={selected ? buffers.current.get(selected) : undefined}
        filterBy={selected ? filterBy[selected] : undefined}
        tauri={tauri}
        onRemove={() => selected && toggle(selected)}
      />
    </div>
  )
}

/** Filter a raw buffer with the TS mirror (browser mock mode only). */
function mirrorFilter(raw: number[], cfg: FilterCfg | undefined, fs: number): number[] {
  if (!cfg || cfg.algo === 'none') return new Array(raw.length).fill(NaN)
  const proc = makeProcessor(cfg.algo, cfg.params, fs)
  return raw.map((v) => proc.process(v))
}

function estimateFs(b: Buffer): number {
  if (b.t.length < 2) return 100
  const dt = (b.t[b.t.length - 1] - b.t[0]) / (b.t.length - 1)
  return dt > 0 ? 1000 / dt : 100
}

/** SET_MESSAGE_INTERVAL options (0 = default rate, -1 = disable). */
const RATE_OPTIONS: Array<{ label: string; intervalUs: number }> = [
  { label: 'Default', intervalUs: 0 },
  { label: '50 Hz', intervalUs: 20_000 },
  { label: '100 Hz', intervalUs: 10_000 },
  { label: '200 Hz', intervalUs: 5_000 },
  { label: '500 Hz', intervalUs: 2_000 },
  { label: 'Disable', intervalUs: -1 },
]

function PropertiesPanel({
  selected,
  catalog,
  buffer,
  filterBy,
  tauri,
  onRemove,
}: {
  selected: string | null
  catalog: CatalogEntry[]
  buffer: Buffer | undefined
  filterBy: FilterCfg | undefined
  tauri: boolean
  onRemove: () => void
}) {
  if (!selected) {
    return (
      <aside className="properties">
        <div className="prop-title">Properties</div>
        <div style={{ color: 'var(--mg-muted)', fontSize: 12 }}>No signal selected — click a plot title.</div>
      </aside>
    )
  }
  const entry = catalog.find((c) => signalKey(c.signal) === selected)
  const fs = buffer ? estimateFs(buffer) : 0
  const messageId = entry?.signal.message_id ?? 30
  return (
    <aside className="properties">
      <div className="prop-title">Properties</div>
      <dl>
        <dt>Signal</dt>
        <dd className="mono">{selected}</dd>
        <dt>Message</dt>
        <dd>{entry ? `msg ${messageId}` : `msg ${messageId} (not seen yet)`}</dd>
        <dt>Rate</dt>
        <dd>{entry ? `${entry.rate_hz.toFixed(1)} Hz` : '—'}</dd>
        <dt>Est. fs</dt>
        <dd>{fs > 0 ? `${fs.toFixed(1)} Hz` : '—'}</dd>
        <dt>Samples</dt>
        <dd>{buffer ? buffer.t.length : 0}</dd>
      </dl>
      <div className="prop-row">
        <label htmlFor="inspector-rate">Stream rate</label>
        <select
          id="inspector-rate"
          disabled={!tauri}
          defaultValue="0"
          onChange={(e) => {
            const intervalUs = Number(e.target.value)
            if (tauri) {
              void invoke('set_message_interval', { messageId, intervalUs }).catch(() => undefined)
            }
          }}
        >
          {RATE_OPTIONS.map((o) => (
            <option key={o.intervalUs} value={o.intervalUs}>
              {o.label}
            </option>
          ))}
        </select>
        {!tauri && <div className="hint">Needs a link (mock data).</div>}
      </div>
      <div className="prop-row">
        <label>Filter</label>
        <span className="mono">{filterBy && filterBy.algo !== 'none' ? filterBy.algo : 'raw'}</span>
      </div>
      <button className="remove" onClick={onRemove}>
        Remove trace
      </button>
    </aside>
  )
}

function SpectrumView({ data, label }: { data: { bins: number[]; peakFreqHz: number; fs: number; deltaF: number } | null; label: string }) {
  if (!data) return <div className="peak-note">{label}: need 1024 samples for FFT</div>
  const n = data.bins.length
  const nyquist = data.fs / 2
  const maxBin = Math.min(n - 1, Math.ceil(nyquist / data.deltaF) || n - 1)
  const x = new Array<number>(maxBin)
  const y = new Array<number>(maxBin)
  for (let i = 0; i < maxBin; i++) {
    x[i] = i * data.deltaF
    y[i] = data.bins[i]
  }
  return (
    <div style={{ fontSize: 11, color: 'var(--mg-muted)', marginTop: 2 }}>
      peak {data.peakFreqHz.toFixed(1)} Hz · fs {data.fs.toFixed(0)} Hz · Δf {data.deltaF.toFixed(2)} Hz
      <div ref={(el) => el && renderSpectrum(el, x, y)} className="chart" />
    </div>
  )
}

function renderSpectrum(host: HTMLDivElement, x: number[], y: number[]): void {
  const chart = new uPlot(
    {
      width: host.clientWidth,
      height: 120,
      legend: { show: false },
      axes: [{ stroke: MUTED() }, { stroke: MUTED() }],
      series: [
        { label: 'Hz', stroke: 'transparent' },
        { label: 'mag', stroke: token('--mg-mag') },
      ],
    },
    [x, y],
    host,
  )
  void chart
}