// Signal Inspector (plan §6, ADR-016): a separate window that buffers the raw
// signal stream into per-trace ring buffers and renders uPlot time plots. The
// document is a set of plot windows; each plot draws one or more traces, and a
// trace binds a signal to a filter chain (optionally an FFT analyzer). In Tauri
// mode the pipeline runs in Rust (`core::inspector::session`, ADR-015) and
// frames arrive columnar, keyed by trace id; in browser mock mode a TS mirror
// of `core::dsp` previews the filters.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import uPlot from 'uplot'
import { invoke, Channel } from '@tauri-apps/api/core'
import { makeProcessor, magnitudeSpectrum, type AlgorithmInfo } from './dsp'
import { isTauri, mockAlgorithms, mockCatalog, mockSamples, msgName, signalKey } from './mock'
import type { AlgoConfig } from '../generated-types/AlgoConfig'
import type { AnalyzerSource } from '../generated-types/AnalyzerSource'
import type { CatalogEntry } from '../generated-types/CatalogEntry'
import type { Plot } from '../generated-types/Plot'
import type { SignalId } from '../generated-types/SignalId'
import type { SpectrumFrame } from '../generated-types/SpectrumFrame'
import type { Trace } from '../generated-types/Trace'

/** Rust `TraceFrame` (inspector_service.rs): columnar, so the SignalId is not
 * repeated per sample. */
interface TraceFrame {
  trace_id: string
  t: number[]
  raw: number[]
  filtered: number[]
}

/** Rust `SampleFrame`. */
interface FramePayload {
  seq: number
  traces: TraceFrame[]
  /** Analyzer windows completed since the last frame (S3: FFT in Rust). */
  spectra: { trace_id: string; frame: SpectrumFrame }[]
}

/** Rust `InspectorStatus` (inspector_service.rs). */
interface InspectorStatus {
  messages: number
  samples: number
  dropped: number
  connected: boolean
  tap_running: boolean
}

interface Buffer {
  t: number[]
  raw: number[]
  filtered: number[]
}

interface Workspace {
  version: 2
  windowSec: number
  plots: Plot[]
  colors: Record<string, string>
  axisLink: boolean
}

const WS_KEY = 'maggcs.inspector.workspace'
const SYNC_KEY = 'maggcs-inspector'

/** Default series palette; a trace picks one by hashing its id, override-able. */
const PALETTE = ['#4ea1ff', '#3ddc84', '#ffb454', '#ff6b6b', '#c792ea', '#22d3ee', '#f472b6', '#a3e635']

function hashIndex(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0
  return Math.abs(h)
}
function colorOf(traceId: string, colors: Record<string, string>): string {
  return colors[traceId] ?? PALETTE[hashIndex(traceId) % PALETTE.length]
}

let idSeq = 0
/** A stable id for a plot/trace within this window (persisted in the workspace). */
function newId(prefix: string): string {
  idSeq += 1
  return `${prefix}${Date.now().toString(36)}${idSeq}`
}

function newTrace(signal: SignalId): Trace {
  return { id: newId('t'), signal, pipeline: [], analyzer: null, analyzer_source: 'raw' }
}

function token(name: string): string {
  const cached = cssVarCache.get(name)
  if (cached) return cached
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim() || 'cyan'
  cssVarCache.set(name, v)
  return v
}
const cssVarCache = new Map<string, string>()
const MUTED = () => token('--mg-muted')

/** The trace with this id, across every plot. */
function findTrace(plots: Plot[], id: string): Trace | undefined {
  for (const p of plots) for (const t of p.traces) if (t.id === id) return t
  return undefined
}

export function InspectorApp() {
  const tauri = isTauri()
  const [catalog, setCatalog] = useState<CatalogEntry[]>([])
  const [algorithms, setAlgorithms] = useState<AlgorithmInfo[]>([])
  const [plots, setPlots] = useState<Plot[]>([])
  const [colors, setColors] = useState<Record<string, string>>({})
  const [paused, setPaused] = useState(false)
  const [windowSec, setWindowSec] = useState(30)
  const [version, setVersion] = useState(0)
  const [selected, setSelected] = useState<string | null>(null)
  const [axisLink, setAxisLink] = useState(false)
  const [status, setStatus] = useState<InspectorStatus | null>(null)
  const [rate, setRate] = useState(0)
  const [notice, setNotice] = useState<string | null>(null)

  const noticeAt = useRef(0)
  const showNotice = useCallback((msg: string) => {
    noticeAt.current = Date.now()
    setNotice(msg)
    setTimeout(() => {
      if (Date.now() - noticeAt.current >= 4000) setNotice(null)
    }, 4000)
  }, [])

  const pausedRef = useRef(paused)
  pausedRef.current = paused
  const windowRef = useRef(windowSec)
  windowRef.current = windowSec
  const plotsRef = useRef(plots)
  plotsRef.current = plots

  const buffers = useRef<Map<string, Buffer>>(new Map())
  /** Latest Rust-computed spectrum per trace (S3). */
  const spectra = useRef<Map<string, SpectrumFrame>>(new Map())
  const charts = useRef<Map<string, uPlot>>(new Map())
  const chartSigs = useRef<Map<string, string>>(new Map())
  const chartHosts = useRef<Map<string, HTMLDivElement>>(new Map())

  const bump = useCallback(() => setVersion((v) => v + 1), [])

  /** checked = every signal that currently has a trace (for the tree). */
  const checked = useMemo(
    () => new Set(plots.flatMap((p) => p.traces.map((t) => signalKey(t.signal)))),
    [plots],
  )

  const patchTrace = useCallback((id: string, patch: (t: Trace) => Trace) => {
    setPlots((prev) =>
      prev.map((p) => ({ ...p, traces: p.traces.map((t) => (t.id === id ? patch(t) : t)) })),
    )
  }, [])

  /** Remove every trace matching `pred`, dropping a plot once it is empty. */
  const removeTracesWhere = useCallback((pred: (t: Trace) => boolean) => {
    setPlots((prev) => {
      const out: Plot[] = []
      for (const p of prev) {
        if (!p.traces.some(pred)) {
          out.push(p)
          continue
        }
        const traces = p.traces.filter((t) => !pred(t))
        if (traces.length > 0) out.push({ ...p, traces })
      }
      return out
    })
  }, [])

  const removeTrace = useCallback((id: string) => removeTracesWhere((t) => t.id === id), [removeTracesWhere])

  const removePlot = useCallback((id: string) => setPlots((prev) => prev.filter((p) => p.id !== id)), [])

  const addPlot = useCallback(() => {
    setPlots((prev) => [...prev, { id: newId('p'), title: `Plot ${prev.length + 1}`, traces: [] }])
  }, [])

  const addTraceToPlot = useCallback((plotId: string, signal: SignalId) => {
    setPlots((prev) =>
      prev.map((p) => (p.id === plotId ? { ...p, traces: [...p.traces, newTrace(signal)] } : p)),
    )
  }, [])

  const setColor = useCallback((traceId: string, color: string) => {
    setColors((prev) => ({ ...prev, [traceId]: color }))
  }, [])

  /** Check/uncheck a signal: add it as a new single-trace plot, or drop it. */
  const toggleSignal = useCallback(
    (signal: SignalId) => {
      const key = signalKey(signal)
      const exists = plotsRef.current.some((p) => p.traces.some((t) => signalKey(t.signal) === key))
      if (exists) {
        removeTracesWhere((t) => signalKey(t.signal) === key)
      } else {
        setPlots((prev) => [...prev, { id: newId('p'), title: key, traces: [newTrace(signal)] }])
      }
    },
    [removeTracesWhere],
  )

  // Workspace save/load (P7 + S2/S3): plots (with full SignalIds + chains),
  // colors, window and axis link. v1 (`checked`/`filterBy` by key) is dropped.
  const saveWorkspace = useCallback(() => {
    const ws: Workspace = { version: 2, windowSec, plots, colors, axisLink }
    localStorage.setItem(WS_KEY, JSON.stringify(ws))
  }, [windowSec, plots, colors, axisLink])

  const loadWorkspace = useCallback(() => {
    const raw = localStorage.getItem(WS_KEY)
    if (!raw) return
    try {
      const ws = JSON.parse(raw) as Workspace
      if (typeof ws.windowSec === 'number') setWindowSec(ws.windowSec)
      if (Array.isArray(ws.plots)) setPlots(ws.plots)
      if (ws.colors && typeof ws.colors === 'object') setColors(ws.colors)
      setAxisLink(Boolean(ws.axisLink))
    } catch {
      // Corrupt workspace: keep the defaults.
    }
  }, [])

  /** Route one live sample to every trace bound to its signal. */
  const routeSample = useCallback(
    (id: SignalId, t_ms: number, raw: number, filtered: number, touched: Set<Buffer>) => {
      const key = signalKey(id)
      for (const p of plotsRef.current) {
        for (const t of p.traces) {
          if (signalKey(t.signal) !== key) continue
          const b = buffers.current.get(t.id) ?? { t: [], raw: [], filtered: [] }
          b.t.push(t_ms)
          b.raw.push(raw)
          b.filtered.push(filtered)
          buffers.current.set(t.id, b)
          touched.add(b)
        }
      }
    },
    [],
  )

  // Data source: the Tauri channel when running in the app, the mock otherwise.
  useEffect(() => {
    loadWorkspace()
    if (!tauri) {
      const id = setInterval(() => {
        const now = performance.now() / 1000
        const touched = new Set<Buffer>()
        for (const s of mockSamples(now)) routeSample(s.id, s.t_ms, s.value, NaN, touched)
        for (const b of touched) trimBuffer(b, windowRef.current, 1024)
        bump()
      }, 10)
      setCatalog(mockCatalog())
      setAlgorithms(mockAlgorithms())
      return () => clearInterval(id)
    }
    let disposed = false
    const channel = new Channel<FramePayload>()
    const onMessage = (frame: FramePayload) => {
      const touched = new Set<Buffer>()
      for (const tf of frame.traces) {
        const b = buffers.current.get(tf.trace_id) ?? { t: [], raw: [], filtered: [] }
        for (let i = 0; i < tf.t.length; i++) {
          b.t.push(tf.t[i])
          // serde_json writes NaN as null; turn it back into NaN (a gap in uPlot).
          b.raw.push(tf.raw[i] ?? NaN)
          b.filtered.push(tf.filtered[i] ?? NaN)
        }
        buffers.current.set(tf.trace_id, b)
        touched.add(b)
      }
      for (const sf of frame.spectra) spectra.current.set(sf.trace_id, sf.frame)
      // The Rust analyzer keeps its own window, so the view buffer only needs
      // the visible span (S3: FFT decoupled from the plot window).
      for (const b of touched) trimBuffer(b, windowRef.current, 2)
      bump()
    }
    channel.onmessage = onMessage
    void (async () => {
      try {
        await invoke('inspector_connect', { channel })
      } catch (e) {
        showNotice(String(e))
      }
      try {
        const cat = await invoke<CatalogEntry[]>('inspector_catalog')
        if (!disposed) setCatalog(cat)
        const algos = await invoke<AlgorithmInfo[]>('inspector_list_algorithms')
        if (!disposed) setAlgorithms(algos)
      } catch (e) {
        showNotice(String(e))
      }
    })()
    return () => {
      disposed = true
      void invoke('inspector_disconnect')
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tauri, bump])

  // Push the flattened trace set (signal + pipeline + analyzer) to the Rust
  // session. An unchanged trace keeps its running state; a changed one is
  // rebuilt, so a filter change restarts that curve (SDI behavior).
  useEffect(() => {
    if (!tauri) return
    const traces = plots.flatMap((p) => p.traces)
    void invoke('inspector_set_traces', { traces }).catch((e) => showNotice(String(e)))
  }, [plots, tauri])

  // Drop buffers/spectra for traces that no longer exist.
  useEffect(() => {
    const live = new Set(plots.flatMap((p) => p.traces.map((t) => t.id)))
    for (const id of [...buffers.current.keys()]) if (!live.has(id)) buffers.current.delete(id)
    for (const id of [...spectra.current.keys()]) if (!live.has(id)) spectra.current.delete(id)
  }, [plots])

  // Refresh the catalog periodically so the tree fills as signals arrive, and
  // poll the tap/link status for the toolbar (dropped, msg/s, link state).
  useEffect(() => {
    if (!tauri) return
    let lastMsgs = 0
    let lastT = 0
    const id = setInterval(async () => {
      try {
        const cat = await invoke<CatalogEntry[]>('inspector_catalog')
        const s = await invoke<InspectorStatus>('inspector_status')
        if (s.tap_running) {
          const now = Date.now()
          if (lastT > 0 && s.messages >= lastMsgs) {
            const dt = (now - lastT) / 1000
            if (dt > 0) setRate((s.messages - lastMsgs) / dt)
          }
          lastMsgs = s.messages
          lastT = now
        } else {
          setRate(0)
        }
        setCatalog(cat)
        setStatus(s)
      } catch {
        /* not connected */
      }
    }, 1000)
    return () => clearInterval(id)
  }, [tauri])

  // Trim every buffer to the selected window (keep at least one FFT window).
  useEffect(() => {
    const keep = Math.max(tauri ? 2 : 1024, Math.ceil((windowSec * 1000) / 10))
    for (const [key, b] of buffers.current) {
      if (b.t.length > keep) {
        b.t = b.t.slice(-keep)
        b.raw = b.raw.slice(-keep)
        b.filtered = b.filtered.slice(-keep)
        buffers.current.set(key, b)
      }
    }
  }, [windowSec, tauri])

  // Create/destroy one uPlot per plot. Recreated when its trace set or the
  // per-trace colors change (series are fixed at construction) or axis linking
  // toggles (the cursor sync key has no runtime setter).
  useEffect(() => {
    for (const p of plots) {
      if (p.traces.length === 0) continue
      const host = chartHosts.current.get(p.id)
      if (!host) continue
      const sig = `${axisLink}|${p.traces.map((t) => `${t.id}:${colorOf(t.id, colors)}`).join(',')}`
      if (charts.current.has(p.id) && chartSigs.current.get(p.id) === sig) continue
      charts.current.get(p.id)?.destroy()
      const cursor = axisLink ? { show: true, sync: { key: SYNC_KEY } } : { show: true }
      const series: uPlot.Series[] = [{ label: 't', stroke: 'transparent' }]
      for (const t of p.traces) {
        const color = colorOf(t.id, colors)
        const name = signalKey(t.signal)
        series.push({ label: `${name} raw`, stroke: color, width: 1, dash: [4, 3] })
        series.push({ label: `${name} filtered`, stroke: color, width: 2 })
      }
      const empty = [[], ...p.traces.flatMap(() => [[], []])] as unknown as uPlot.AlignedData
      const chart = new uPlot(
        {
          width: host.clientWidth,
          height: 180,
          legend: { show: true },
          cursor,
          scales: { x: { time: false } },
          axes: [
            { stroke: MUTED(), grid: { stroke: MUTED(), width: 1, dash: [2, 4] } },
            { stroke: MUTED() },
          ],
          series,
        },
        empty,
        host,
      )
      charts.current.set(p.id, chart)
      chartSigs.current.set(p.id, sig)
    }
    for (const id of [...charts.current.keys()]) {
      if (!plots.some((p) => p.id === id && p.traces.length > 0)) {
        charts.current.get(id)?.destroy()
        charts.current.delete(id)
        chartSigs.current.delete(id)
      }
    }
  }, [plots, axisLink, colors])

  // Feed data into the charts whenever new samples arrive or filters change.
  // Paused = freeze the view (SDI behavior): data keeps buffering above, but the
  // visible window stops advancing until Run.
  useEffect(() => {
    if (pausedRef.current) return
    for (const p of plots) {
      const chart = charts.current.get(p.id)
      if (!chart) continue
      const merged = mergedData(p, buffers.current, tauri)
      if (!merged) continue
      chart.setData(merged.data)
      if (merged.tEnd > 0) chart.setScale('x', { min: merged.tEnd - windowRef.current, max: merged.tEnd })
    }
  }, [version, plots, tauri, paused])

  useEffect(() => {
    for (const chart of charts.current.values()) chart.redraw()
  }, [windowSec])

  const clearAll = () => {
    buffers.current.clear()
    for (const chart of charts.current.values()) chart.setData(chart.series.map(() => []) as unknown as uPlot.AlignedData)
  }

  const exportCsv = () => {
    const rows: string[] = ['t_ms,trace,raw,filtered']
    for (const p of plots) {
      for (const t of p.traces) {
        const b = buffers.current.get(t.id)
        if (!b) continue
        for (let i = 0; i < b.t.length; i++) {
          rows.push(`${b.t[i].toFixed(1)},${signalKey(t.signal)},${b.raw[i]},${b.filtered[i] ?? ''}`)
        }
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

  /** Latest spectrum for a trace: computed in Rust in the app (decoupled from
   * the view window), the TS mirror in mock mode. */
  const spectrum = (trace: Trace): SpectrumFrame | null => {
    if (tauri) return spectra.current.get(trace.id) ?? null
    const b = buffers.current.get(trace.id)
    if (!b) return null
    const n = 1024
    const recent = b.raw.slice(-n)
    if (recent.length < n) return null
    const fs = estimateFs(b)
    const s = magnitudeSpectrum(recent, fs)
    return s
      ? {
          fs,
          n: s.n,
          delta_f: s.deltaF,
          nyquist: s.nyquist,
          bins: s.bins,
          peak_bin: s.peakBin,
          peak_freq_hz: s.peakFreqHz,
          peak_value: s.peakValue,
        }
      : null
  }

  /** FFT button: attach/detach the analyzer on a trace (runs in Rust in the
   * app; the TS mirror is used in mock mode). */
  const toggleFft = useCallback(
    (id: string) => {
      patchTrace(id, (t) => {
        if (t.analyzer) return { ...t, analyzer: null }
        const a = algorithms.find((x) => x.kind === 'analyzer')
        const params: [string, number][] = a ? a.params.map((pp) => [pp.key, pp.default]) : [['n', 1024]]
        return { ...t, analyzer: { algorithm: a?.id ?? 'fft', params } }
      })
    },
    [algorithms, patchTrace],
  )

  /** Append a filter stage (the pipeline is multi-stage: detrend → LPF → …). */
  const addStage = useCallback(
    (id: string) => {
      const a = algorithms.find((x) => x.kind === 'processor')
      if (!a) return
      const params: [string, number][] = a.params.map((pp) => [pp.key, pp.default])
      patchTrace(id, (t) => ({ ...t, pipeline: [...t.pipeline, { algorithm: a.id, params }] }))
    },
    [algorithms, patchTrace],
  )

  const removeStage = useCallback(
    (id: string, index: number) =>
      patchTrace(id, (t) => ({ ...t, pipeline: t.pipeline.filter((_, i) => i !== index) })),
    [patchTrace],
  )

  const setStage = useCallback(
    (id: string, index: number, next: AlgoConfig) =>
      patchTrace(id, (t) => ({ ...t, pipeline: t.pipeline.map((s, i) => (i === index ? next : s)) })),
    [patchTrace],
  )

  const selectedTrace = selected ? findTrace(plots, selected) : undefined

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
        <button onClick={addPlot}>New plot</button>
        <button onClick={clearAll}>Clear data</button>
        <button onClick={exportCsv}>Export CSV</button>
        <button onClick={saveWorkspace}>Save</button>
        <button onClick={loadWorkspace}>Load</button>
        <span style={{ marginLeft: 'auto', color: 'var(--mg-muted)', fontSize: 11 }}>
          {tauri && status && (
            <>
              {status.connected ? <span style={{ color: 'var(--mg-ok)' }}>● link</span> : <span style={{ color: 'var(--mg-warn)' }}>● no link</span>}
              {' · '}
              {status.tap_running ? `${rate.toFixed(0)} msg/s` : '—'} · dropped {status.dropped}
            </>
          )}
          {'  '}Signal Inspector{tauri ? '' : ' · mock'}
        </span>
      </div>
      {notice && <div className="inspector-notice">{notice}</div>}

      <SignalBrowser catalog={catalog} checked={checked} onToggle={toggleSignal} />

      <div className="plot-grid">
        {plots.map((p) => (
          <div className="plot" key={p.id}>
            <div className="plot-head">
              <span className="title">{p.title || 'Plot'}</span>
              <select
                value=""
                title="Add a signal to this plot"
                onChange={(e) => {
                  const entry = catalog.find((c) => signalKey(c.signal) === e.target.value)
                  if (entry) addTraceToPlot(p.id, entry.signal)
                }}
              >
                <option value="">+ Signal</option>
                {catalog
                  .filter((c) => !p.traces.some((t) => signalKey(t.signal) === signalKey(c.signal)))
                  .map((c) => (
                    <option key={signalKey(c.signal)} value={signalKey(c.signal)}>
                      {c.message_name}.{c.signal.field}
                    </option>
                  ))}
              </select>
              <button title="Remove plot" onClick={() => removePlot(p.id)}>
                ×
              </button>
            </div>
            {p.traces.length === 0 ? (
              <div className="peak-note">Empty plot — add a signal.</div>
            ) : (
              <div
                ref={(el) => {
                  if (el) chartHosts.current.set(p.id, el)
                  else chartHosts.current.delete(p.id)
                }}
                className="chart"
              />
            )}
            <div className="trace-chips">
              {p.traces.map((t) => (
                <span className={`trace-chip ${selected === t.id ? 'selected' : ''}`} key={t.id}>
                  <input
                    type="color"
                    title="Series color"
                    value={colorOf(t.id, colors)}
                    onChange={(e) => setColor(t.id, e.target.value)}
                  />
                  <span className="mono" onClick={() => setSelected(t.id)}>
                    {signalKey(t.signal)}
                  </span>
                  <button className={t.analyzer ? 'active' : ''} onClick={() => toggleFft(t.id)}>
                    FFT
                  </button>
                  {t.analyzer && (
                    <select
                      value={t.analyzer_source}
                      title="Analyzer input"
                      onChange={(e) => {
                        const src = e.target.value as AnalyzerSource
                        patchTrace(t.id, (tr) => ({ ...tr, analyzer_source: src }))
                      }}
                    >
                      <option value="raw">raw</option>
                      <option value="filtered">filtered</option>
                    </select>
                  )}
                  <button title="Remove trace" onClick={() => removeTrace(t.id)}>
                    ×
                  </button>
                </span>
              ))}
            </div>
            {selected && p.traces.some((t) => t.id === selected) && selectedTrace?.analyzer && (
              <SpectrumView frame={spectrum(selectedTrace)} label={signalKey(selectedTrace.signal)} />
            )}
          </div>
        ))}
      </div>

      <PropertiesPanel
        selected={selected}
        trace={selectedTrace}
        color={selected ? colorOf(selected, colors) : undefined}
        catalog={catalog}
        algorithms={algorithms}
        buffer={selected ? buffers.current.get(selected) : undefined}
        tauri={tauri}
        onColor={(c) => selected && setColor(selected, c)}
        onRemove={() => selected && removeTrace(selected)}
        onAddStage={() => selected && addStage(selected)}
        onRemoveStage={(i) => selected && removeStage(selected, i)}
        onSetStage={(i, next) => selected && setStage(selected, i, next)}
        onError={showNotice}
      />
    </div>
  )
}

/** Left panel: catalog grouped by message with a search box and collapsible
 * groups. Grouping makes the tree usable with PX4's hundreds of fields. */
function SignalBrowser({
  catalog,
  checked,
  onToggle,
}: {
  catalog: CatalogEntry[]
  checked: Set<string>
  onToggle: (signal: SignalId) => void
}) {
  const [query, setQuery] = useState('')
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set())
  const q = query.trim().toLowerCase()

  const groups = useMemo(() => {
    const byMsg = new Map<string, CatalogEntry[]>()
    for (const c of catalog) {
      const name = c.message_name || msgName(c.signal.message_id)
      const arr = byMsg.get(name) ?? []
      arr.push(c)
      byMsg.set(name, arr)
    }
    const rows = [...byMsg.entries()].map(([name, entries]) => ({
      name,
      entries: entries.sort((a, b) => a.signal.field.localeCompare(b.signal.field)),
    }))
    rows.sort((a, b) => a.name.localeCompare(b.name))
    return rows
  }, [catalog])

  const visible = useMemo(() => {
    if (!q) return groups
    return groups
      .map((g) => ({
        ...g,
        entries: g.entries.filter(
          (c) => c.signal.field.toLowerCase().includes(q) || g.name.toLowerCase().includes(q),
        ),
      }))
      .filter((g) => g.entries.length > 0)
  }, [groups, q])

  const toggleGroup = (name: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })

  return (
    <div className="signal-list">
      <div style={{ marginBottom: 6, fontWeight: 600 }}>Signals</div>
      <input
        className="signal-search"
        placeholder="Search…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      {catalog.length === 0 && <div style={{ color: 'var(--mg-muted)' }}>Waiting for data…</div>}
      {visible.map((g) => {
        const isCollapsed = collapsed.has(g.name)
        const rateSum = g.entries.reduce((s, c) => s + (Number.isFinite(c.rate_hz) ? c.rate_hz : 0), 0)
        return (
          <div className="signal-group" key={g.name}>
            <div className="signal-group-head" onClick={() => toggleGroup(g.name)}>
              <span className={isCollapsed ? 'caret caret-closed' : 'caret'} />
              <span>{g.name}</span>
              <span className="msg">
                {g.entries.length} · {fmt(rateSum, 0)} Hz
              </span>
            </div>
            {!isCollapsed &&
              g.entries.map((c) => {
                const key = signalKey(c.signal)
                return (
                  <label key={key}>
                    <input type="checkbox" checked={checked.has(key)} onChange={() => onToggle(c.signal)} />
                    <span className="mono">{c.signal.field}</span>
                    <span className="msg">
                      {' '}
                      {fmt(c.rate_hz, 0)} Hz · {fmt(c.last_value, 1)}
                    </span>
                  </label>
                )
              })}
          </div>
        )
      })}
    </div>
  )
}

/** Filter a raw buffer with the TS mirror (browser mock mode only); applies the
 * trace's whole pipeline in order. */
function mirrorFilter(raw: number[], pipeline: AlgoConfig[], fs: number): number[] {
  if (pipeline.length === 0) return new Array(raw.length).fill(NaN)
  const procs = pipeline.map((s) => makeProcessor(s.algorithm, Object.fromEntries(s.params) as Record<string, number>, fs))
  return raw.map((v) => {
    let x = v
    for (const p of procs) x = p.process(x)
    return x
  })
}

/** Merge every trace in a plot onto one sorted time axis (uPlot needs each
 * series to share x), padding each series with NaN where it has no sample. */
function mergedData(
  plot: Plot,
  buffers: Map<string, Buffer>,
  tauri: boolean,
): { data: uPlot.AlignedData; tEnd: number } | null {
  const columns: { t: number[]; raw: number[]; filt: number[] }[] = []
  const stamps = new Set<number>()
  for (const tr of plot.traces) {
    const b = buffers.get(tr.id)
    if (!b || b.t.length === 0) {
      columns.push({ t: [], raw: [], filt: [] })
      continue
    }
    const filt = tauri ? b.filtered : mirrorFilter(b.raw, tr.pipeline, estimateFs(b))
    for (const x of b.t) stamps.add(x)
    columns.push({ t: b.t, raw: b.raw, filt })
  }
  const xs = [...stamps].sort((a, b) => a - b)
  if (xs.length < 2) return null
  const x = Float64Array.from(xs, (v) => v / 1000)
  const series: Float64Array[] = [x]
  for (const c of columns) {
    const rawMap = new Map<number, number>()
    const filtMap = new Map<number, number>()
    for (let i = 0; i < c.t.length; i++) {
      rawMap.set(c.t[i], c.raw[i])
      filtMap.set(c.t[i], c.filt[i])
    }
    series.push(Float64Array.from(xs, (v) => finiteOrNaN(rawMap.get(v))))
    series.push(Float64Array.from(xs, (v) => finiteOrNaN(filtMap.get(v))))
  }
  // `tEnd` is in seconds to match the x data (uPlot scales use the data units).
  return { data: series as uPlot.AlignedData, tEnd: xs[xs.length - 1] / 1000 }
}

function finiteOrNaN(v: number | undefined): number {
  return v !== undefined && Number.isFinite(v) ? v : NaN
}

/** Number formatter that tolerates null/NaN (serde_json turns NaN into null). */
function fmt(v: number | null | undefined, digits: number): string {
  return typeof v === 'number' && Number.isFinite(v) ? v.toFixed(digits) : '—'
}

/** Drop samples older than the window (relative to the newest sample), but
 * never below `minSamples` (the FFT window in mock mode). */
function trimBuffer(b: Buffer, windowSec: number, minSamples: number): void {
  const n = b.t.length
  if (n < 2) return
  const keep = Math.max(minSamples, Math.ceil((windowSec * 1000) / 10))
  const maxDrop = Math.max(0, n - keep)
  const cutoff = b.t[n - 1] - windowSec * 1000
  let k = 0
  while (k < maxDrop && b.t[k] < cutoff) k++
  if (k > 0) {
    b.t.splice(0, k)
    b.raw.splice(0, k)
    b.filtered.splice(0, k)
  }
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
  trace,
  color,
  catalog,
  algorithms,
  buffer,
  tauri,
  onColor,
  onRemove,
  onAddStage,
  onRemoveStage,
  onSetStage,
  onError,
}: {
  selected: string | null
  trace: Trace | undefined
  color: string | undefined
  catalog: CatalogEntry[]
  algorithms: AlgorithmInfo[]
  buffer: Buffer | undefined
  tauri: boolean
  onColor: (color: string) => void
  onRemove: () => void
  onAddStage: () => void
  onRemoveStage: (index: number) => void
  onSetStage: (index: number, next: AlgoConfig) => void
  onError: (msg: string) => void
}) {
  if (!selected || !trace) {
    return (
      <aside className="properties">
        <div className="prop-title">Properties</div>
        <div style={{ color: 'var(--mg-muted)', fontSize: 12 }}>No trace selected — click a trace chip.</div>
      </aside>
    )
  }
  const key = signalKey(trace.signal)
  const entry = catalog.find((c) => signalKey(c.signal) === key)
  const fs = buffer ? estimateFs(buffer) : 0
  const messageId = entry?.signal.message_id ?? trace.signal.message_id
  return (
    <aside className="properties">
      <div className="prop-title">Properties</div>
      <dl>
        <dt>Signal</dt>
        <dd className="mono">{key}</dd>
        <dt>Message</dt>
        <dd>{entry ? `msg ${messageId}` : `msg ${messageId} (not seen yet)`}</dd>
        <dt>Rate</dt>
        <dd>{entry ? `${fmt(entry.rate_hz, 1)} Hz` : '—'}</dd>
        <dt>Est. fs</dt>
        <dd>{fs > 0 ? `${fs.toFixed(1)} Hz` : '—'}</dd>
        <dt>Samples</dt>
        <dd>{buffer ? buffer.t.length : 0}</dd>
      </dl>
      <div className="prop-row">
        <label>Color</label>
        <input type="color" value={color} onChange={(e) => onColor(e.target.value)} />
      </div>
      <div className="prop-row">
        <label htmlFor="inspector-rate">Stream rate</label>
        <select
          id="inspector-rate"
          disabled={!tauri}
          defaultValue="0"
          onChange={(e) => {
            const intervalUs = Number(e.target.value)
            if (tauri) {
              void invoke('set_message_interval', { messageId, intervalUs }).catch((e) => onError(String(e)))
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
      <div className="prop-title">Filter chain</div>
      {trace.pipeline.length === 0 && <div className="hint">Raw (no filter).</div>}
      {trace.pipeline.map((stage, si) => {
        const info = algorithms.find((a) => a.id === stage.algorithm)
        return (
          <div className="prop-row" key={si}>
            <select
              value={stage.algorithm}
              onChange={(e) => {
                const a = algorithms.find((x) => x.id === e.target.value)
                const params: [string, number][] = a ? a.params.map((pp) => [pp.key, pp.default]) : []
                onSetStage(si, { algorithm: e.target.value, params })
              }}
            >
              {algorithms
                .filter((a) => a.kind === 'processor')
                .map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name}
                  </option>
                ))}
            </select>
            <button title="Remove stage" onClick={() => onRemoveStage(si)}>
              ×
            </button>
            {info?.params.map((pp) => {
              const cur = stage.params.find(([k]) => k === pp.key)?.[1] ?? pp.default
              return (
                <label key={pp.key} style={{ fontSize: 11 }}>
                  {pp.label}
                  <input
                    type="number"
                    style={{ width: 64, background: 'var(--mg-bg)', color: 'var(--mg-ink)', border: '1px solid var(--mg-border)', borderRadius: 4 }}
                    value={cur}
                    onChange={(e) => {
                      const v = Number(e.target.value)
                      onSetStage(si, {
                        ...stage,
                        params: stage.params.map(([k, old]) =>
                          k === pp.key ? ([k, v] as [string, number]) : ([k, old] as [string, number]),
                        ),
                      })
                    }}
                  />
                </label>
              )
            })}
          </div>
        )
      })}
      <button onClick={onAddStage}>+ Add filter stage</button>
      <button className="remove" onClick={onRemove}>
        Remove trace
      </button>
    </aside>
  )
}

/** FFT view: one persistent uPlot instance (created on mount, destroyed on
 * unmount) updated via setData — the old inline-ref version created a new
 * chart on every 30 Hz render and leaked DOM. */
function SpectrumView({ frame, label }: { frame: SpectrumFrame | null; label: string }) {
  const hostRef = useRef<HTMLDivElement | null>(null)
  const plotRef = useRef<uPlot | null>(null)
  const [ready, setReady] = useState(false)

  const setHost = useCallback((el: HTMLDivElement | null) => {
    hostRef.current = el
    if (el) {
      if (!plotRef.current) {
        plotRef.current = new uPlot(
          {
            width: el.clientWidth,
            height: 120,
            legend: { show: false },
            axes: [{ stroke: MUTED() }, { stroke: MUTED() }],
            series: [
              { label: 'Hz', stroke: 'transparent' },
              { label: 'mag', stroke: token('--mg-mag') },
            ],
          },
          [[], []],
          el,
        )
        setReady(true)
      }
    } else {
      plotRef.current?.destroy()
      plotRef.current = null
      setReady(false)
    }
  }, [])

  useEffect(() => () => {
    plotRef.current?.destroy()
    plotRef.current = null
  }, [])

  useEffect(() => {
    const plot = plotRef.current
    if (!plot || !frame || frame.bins.length === 0 || frame.delta_f <= 0) return
    // Draw up to Nyquist (the FFT returns mirrored bins above it).
    const maxBin = Math.max(1, Math.min(frame.bins.length - 1, Math.round(frame.nyquist / frame.delta_f)))
    const x = new Array<number>(maxBin)
    const y = new Array<number>(maxBin)
    for (let i = 0; i < maxBin; i++) {
      x[i] = i * frame.delta_f
      y[i] = frame.bins[i]
    }
    plot.setData([x, y] as uPlot.AlignedData)
  }, [frame])

  if (!frame) return <div className="peak-note">{label}: waiting for a full FFT window…</div>
  return (
    <div style={{ fontSize: 11, color: 'var(--mg-muted)', marginTop: 2 }}>
      {label} · peak {frame.peak_freq_hz.toFixed(1)} Hz · fs {frame.fs.toFixed(0)} Hz · Nyquist {frame.nyquist.toFixed(0)} Hz · Δf {frame.delta_f.toFixed(2)} Hz
      <div ref={setHost} className="chart" />
      {!ready && <div className="peak-note">allocating chart…</div>}
    </div>
  )
}
