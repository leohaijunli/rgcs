// Signal Inspector (plan §6, ADR-016): a separate window that buffers the raw
// signal stream into ring buffers and renders uPlot time plots with optional
// filtered traces (P5) and a spectrum view (P6), plus SDI-style controls (P7:
// run/pause, window duration, clear, cursor, CSV export).

import { useCallback, useEffect, useRef, useState } from 'react'
import uPlot from 'uplot'
import { invoke, Channel } from '@tauri-apps/api/core'
import { makeProcessor, magnitudeSpectrum, type AlgorithmInfo } from './dsp'
import { isTauri, mockCatalog, mockSamples, signalKey } from './mock'
import type { SignalSample } from '../generated-types/SignalSample'
import type { CatalogEntry } from '../generated-types/CatalogEntry'

interface FramePayload {
  seq: number
  samples: SignalSample[]
}

interface Buffer {
  t: number[]
  x: number[]
}

function token(name: string): string {
  const cached = cssVarCache.get(name)
  if (cached) return cached
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim() || '#22d3ee'
  cssVarCache.set(name, v)
  return v
}
const cssVarCache = new Map<string, string>()
const ACCENT = () => token('--mg-accent')
const MUTED = () => token('--mg-muted')

export function InspectorApp() {
  const tauri = isTauri()
  const [catalog, setCatalog] = useState<CatalogEntry[]>([])
  const [algorithms, setAlgorithms] = useState<AlgorithmInfo[]>([])
  const [checked, setChecked] = useState<Set<string>>(new Set())
  const [paused, setPaused] = useState(false)
  const [windowSec, setWindowSec] = useState(30)
  const [version, setVersion] = useState(0)
  const [spectrumFor, setSpectrumFor] = useState<string | null>(null)
  const [filterBy, setFilterBy] = useState<Record<string, { algo: string; params: Record<string, number> }>>({})

  const buffers = useRef<Map<string, Buffer>>(new Map())
  const plots = useRef<Map<string, uPlot>>(new Map())
  const processors = useRef<Map<string, ReturnType<typeof makeProcessor>>>(new Map())
  const plotHosts = useRef<Map<string, HTMLDivElement>>(new Map())

  const bump = useCallback(() => setVersion((v) => v + 1), [])

  // Data source: the Tauri channel when running in the app, the mock otherwise.
  useEffect(() => {
    if (!tauri) {
      const id = setInterval(() => {
        if (paused) return
        const now = performance.now() / 1000
        for (const s of mockSamples(now)) {
          const key = signalKey(s.id)
          const b = buffers.current.get(key) ?? { t: [], x: [] }
          b.t.push(s.t_ms)
          b.x.push(s.value)
          buffers.current.set(key, b)
        }
        bump()
      }, 10)
      void mockCatalog
      return () => clearInterval(id)
    }
    let disposed = false
    const channel = new Channel<FramePayload>()
    const onMessage = (frame: FramePayload) => {
      if (paused) return
      for (const s of frame.samples) {
        const key = signalKey(s.id)
        const b = buffers.current.get(key) ?? { t: [], x: [] }
        b.t.push(s.t_ms)
        b.x.push(s.value)
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
  }, [tauri, paused, bump])

  // Push the union of checked signals to the tap's subscription set.
  useEffect(() => {
    if (!tauri) return
    if (checked.size === 0) {
      void invoke('inspector_subscribe', { signals: [] })
      return
    }
    const signals = [...checked].map((key) => {
      const entry = catalog.find((c) => signalKey(c.signal) === key)
      return entry
        ? entry.signal
        : { system_id: 1, component_id: 1, message_id: 30, field: key.split('.').pop() ?? key }
    })
    void invoke('inspector_subscribe', { signals })
  }, [checked, tauri, catalog])

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
        b.x = b.x.slice(-keep)
        buffers.current.set(key, b)
      }
    }
  }, [windowSec])

  // Render/update the checked plots.
  useEffect(() => {
    // Create plots for newly checked signals.
    for (const key of checked) {
      if (plots.current.has(key)) continue
      const host = plotHosts.current.get(key)
      if (!host) continue
      const plot = new uPlot(
        {
          width: host.clientWidth,
          height: 180,
          legend: { show: true },
          cursor: { show: true },
          axes: [{ stroke: MUTED(), grid: { stroke: 'rgba(255,255,255,0.06)' } }, { stroke: MUTED() }],
          series: [
            { label: 't', stroke: 'transparent' },
            { label: 'raw', stroke: ACCENT() },
            { label: 'filtered', stroke: '#22c55e', width: 2 },
          ],
        },
        [[], []],
        host,
      )
      plots.current.set(key, plot)
    }
    // Drop plots for unchecked signals.
    for (const key of [...plots.current.keys()]) {
      if (!checked.has(key)) {
        plots.current.get(key)?.destroy()
        plots.current.delete(key)
        processors.current.delete(key)
      }
    }
  }, [checked])

  // Feed data into the plots whenever new samples arrive or filters change.
  useEffect(() => {
    for (const key of checked) {
      const plot = plots.current.get(key)
      const b = buffers.current.get(key)
      if (!plot || !b || b.t.length < 2) continue
      const cfg = filterBy[key]
      let proc: ReturnType<typeof makeProcessor> | null = null
      if (cfg && cfg.algo !== 'none') {
        proc = processors.current.get(key) ?? makeProcessor(cfg.algo, cfg.params, estimateFs(b))
        processors.current.set(key, proc)
        proc.configure(cfg.params, estimateFs(b))
      }
      const raw = b.x
      const filt = proc ? raw.map((v) => proc!.process(v)) : []
      const data: Float64Array[] = [Float64Array.from(b.t), Float64Array.from(b.x)]
      if (proc) data.push(Float64Array.from(filt))
      else data.push(new Float64Array(b.t.length).fill(NaN))
      plot.setData(data as uPlot.AlignedData)
    }
  }, [version, checked, filterBy])

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
    const rows: string[] = ['t_ms,key,value']
    for (const [key, b] of buffers.current) {
      for (let i = 0; i < b.t.length; i++) rows.push(`${b.t[i].toFixed(1)},${key},${b.x[i]}`)
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
    const recent = b.x.slice(-n)
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
        <button onClick={() => setChecked((p) => new Set(p))}>Clear</button>
        <button onClick={clearAll}>Clear data</button>
        <button onClick={exportCsv}>Export CSV</button>
        <span style={{ marginLeft: 'auto', color: 'var(--mg-muted)', fontSize: 11 }}>Signal Inspector{tauri ? '' : ' · mock'}</span>
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
              <span className="title">{key}</span>
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
    </div>
  )
}

function estimateFs(b: Buffer): number {
  if (b.t.length < 2) return 100
  const dt = (b.t[b.t.length - 1] - b.t[0]) / (b.t.length - 1)
  return dt > 0 ? 1000 / dt : 100
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
        { label: 'mag', stroke: '#c084fc' },
      ],
    },
    [x, y],
    host,
  )
  void chart
}