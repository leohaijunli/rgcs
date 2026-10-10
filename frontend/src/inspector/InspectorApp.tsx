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
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import {
  ChevronsRight,
  Eraser,
  FolderOpen,
  Link2,
  Maximize2,
  MoreHorizontal,
  Pause,
  Play,
  Plus,
  Radio,
  Ruler,
} from 'lucide-react'
import { makeProcessor, magnitudeSpectrum, type AlgorithmInfo } from './dsp'
import { isTauri, mockAlgorithms, mockCatalog, mockSamples, msgName, signalKey } from './mock'
import type { AlgoConfig } from '../generated-types/AlgoConfig'
import type { AnalyzerSource } from '../generated-types/AnalyzerSource'
import type { CatalogEntry } from '../generated-types/CatalogEntry'
import type { Plot } from '../generated-types/Plot'
import type { SignalId } from '../generated-types/SignalId'
import type { SpectrumFrame } from '../generated-types/SpectrumFrame'
import type { Trace } from '../generated-types/Trace'
import { Ring } from './ring'

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

/** Rust `ReplayStatus` (inspector_service.rs): ULog playback state. */
interface ReplayStatus {
  active: boolean
  name: string | null
  duration_ms: number
  position_ms: number
  playing: boolean
  speed: number
}

/** Rust `UlogInfo` (inspector_service.rs): result of opening a `.ulg`. */
interface UlogInfo {
  name: string
  duration_ms: number
  topics: { name: string; signals: number }[]
}

/** Replay speed presets (matches the Rust `ReplayControl` clamp). */
const REPLAY_SPEEDS = [0.25, 0.5, 1, 2, 4, 8]

/** `m:ss.s` clock for the replay slider. */
function fmtClock(sec: number): string {
  if (!Number.isFinite(sec) || sec < 0) return '0:00.0'
  const m = Math.floor(sec / 60)
  const s = sec - m * 60
  return `${m}:${s.toFixed(1).padStart(4, '0')}`
}

/** Per-trace ring capacity (~5 min at 200 Hz); older samples are overwritten. */
const RING_CAP = 65536

/** Catalog/status poll period. The tree only needs a coarse refresh, so we
 * throttle the IPC+React churn here rather than in Rust: `catalog.observe`
 * still runs on every message, keeping `rate_hz`'s EMA accurate (S4). */
const CATALOG_POLL_MS = 1000

interface Workspace {
  version: 2
  windowSec: number
  plots: Plot[]
  colors: Record<string, string>
  axisLink: boolean
  layoutCols?: number
}

const WS_KEY = 'maggcs.inspector.workspace'
const SYNC_KEY = 'maggcs-inspector'

/** Default series palette; a trace picks one by hashing its id, override-able. */
const PALETTE = ['#4ea1ff', '#3ddc84', '#ffb454', '#ff6b6b', '#c792ea', '#22d3ee', '#f472b6', '#a3e635']

/** Pinned-cursor line colour for the dual-cursor measurement (P7). */
const MEASURE_COLOR = '#ffb454'

/** uPlot axis font / label font, kept at the panel text size so ticks and units
 * stay legible in the narrow spectrum view. */
const AXIS_FONT = '11px system-ui, sans-serif'

/** Compact magnitude tick label: scientific notation only when the numbers get
 * long, so the y-axis gutter stays narrow enough to leave room for the curve. */
function fmtMag(v: number): string {
  if (v === 0) return '0'
  const a = Math.abs(v)
  if (a >= 1e4 || a < 1e-3) return v.toExponential(1)
  return String(Number(v.toPrecision(3)))
}

/** Default parameters for a new filter stage. The registry default low-pass
 * cutoff is 5 Hz, which sits above Nyquist for slow MAVLink streams (a 5 Hz
 * filter on a 10 Hz signal cannot be designed), so pull `fc_hz` under the
 * observed rate — otherwise the stage would silently be dropped by the DSP. */
function defaultStageParams(a: AlgorithmInfo, rate_hz: number): [string, number][] {
  return a.params.map((pp) => {
    const v = pp.key === 'fc_hz' && rate_hz > 0 ? Math.min(pp.default, 0.4 * rate_hz) : pp.default
    return [pp.key, v] as [string, number]
  })
}

function hashIndex(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0
  return Math.abs(h)
}
/** Series colour for a signal (SDI colours each signal, not each trace). */
function colorOf(signalKey_: string, colors: Record<string, string>): string {
  return colors[signalKey_] ?? PALETTE[hashIndex(signalKey_) % PALETTE.length]
}

function hexToRgb(hex: string): [number, number, number] | null {
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(hex.trim())
  if (!m) return null
  const h = m[1].length === 3 ? m[1].replace(/./g, (c) => c + c) : m[1]
  const n = parseInt(h, 16)
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255]
}

function rgbToHsl(r: number, g: number, b: number): [number, number, number] {
  const rr = r / 255
  const gg = g / 255
  const bb = b / 255
  const max = Math.max(rr, gg, bb)
  const min = Math.min(rr, gg, bb)
  const l = (max + min) / 2
  const d = max - min
  if (d === 0) return [0, 0, l]
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
  let h: number
  if (max === rr) h = ((gg - bb) / d + (gg < bb ? 6 : 0)) / 6
  else if (max === gg) h = ((bb - rr) / d + 2) / 6
  else h = ((rr - gg) / d + 4) / 6
  return [h, s, l]
}

function hslToHex(h: number, s: number, l: number): string {
  const channel = (n: number) => {
    const k = (n + h * 12) % 12
    const a = s * Math.min(l, 1 - l)
    const v = l - a * Math.max(-1, Math.min(k - 3, Math.min(9 - k, 1)))
    return Math.round(v * 255).toString(16).padStart(2, '0')
  }
  return `#${channel(0)}${channel(8)}${channel(4)}`
}

/** Colour for the derived "filtered" curve: the signal hue rotated ~150° and
 * re-lit, so the filter output reads apart from the signal-coloured raw trace
 * (the raw line keeps the colour the user picked). */
function filteredColor(base: string): string {
  const rgb = hexToRgb(base)
  if (!rgb) return base
  const [h, s, l] = rgbToHsl(rgb[0], rgb[1], rgb[2])
  const nh = (h + 0.42) % 1
  const nl = l < 0.5 ? Math.min(0.8, l + 0.18) : Math.max(0.4, l - 0.1)
  return hslToHex(nh, Math.min(1, s + 0.05), nl)
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
/** Body text colour: dark on the light theme, bright on the dark theme, so
 * labels and tick values stay legible on either background. */
const INK = () => token('--mg-ink')

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
  const [selected, setSelected] = useState<string | null>(null)
  const [axisLink, setAxisLink] = useState(false)
  /** Plot-grid column count (P7 layout presets): 1×1 / 2×1 / 3×1. */
  const [layoutCols, setLayoutCols] = useState(1)
  /** SDI-style dual cursor: click a plot to pin cursor A for Δt/Δy (P7). */
  const [measure, setMeasure] = useState(false)
  /** Plot that checkboxes add signals to (the "same plot" workflow). */
  const [activePlot, setActivePlot] = useState<string | null>(null)
  /** Auto-scroll x to the newest data; a user drag-zoom turns it off. */
  const [follow, setFollow] = useState(true)
  const [status, setStatus] = useState<InspectorStatus | null>(null)
  const [rate, setRate] = useState(0)
  const [notice, setNotice] = useState<string | null>(null)
  /** Plot currently highlighted as a drag-and-drop target (P7). */
  const [dropPlot, setDropPlot] = useState<string | null>(null)
  /** Overflow menu (export / save / load) open state. */
  const [menuOpen, setMenuOpen] = useState(false)
  /** ULog playback state (`null` = the live link is the source). */
  const [replay, setReplay] = useState<ReplayStatus | null>(null)
  /** Seek slider position while dragging; committed to the backend on release. */
  const [seekPreview, setSeekPreview] = useState<number | null>(null)

  const noticeAt = useRef(0)
  const showNotice = useCallback((msg: string) => {
    noticeAt.current = Date.now()
    setNotice(msg)
    setTimeout(() => {
      if (Date.now() - noticeAt.current >= 4000) setNotice(null)
    }, 4000)
  }, [])

  /** Open a `.ulg` and switch the source to replay (plan decision B). */
  const openUlog = useCallback(async () => {
    try {
      const path = await openDialog({
        multiple: false,
        filters: [{ name: 'ULog', extensions: ['ulg'] }],
      })
      if (typeof path !== 'string') return
      const info = await invoke<UlogInfo>('inspector_open_ulog', { path })
      setReplay({
        active: true,
        name: info.name,
        duration_ms: info.duration_ms,
        position_ms: 0,
        playing: true,
        speed: 1,
      })
      showNotice(
        `Replaying ${info.name} · ${(info.duration_ms / 1000).toFixed(1)} s · ${info.topics.length} topics`,
      )
    } catch (e) {
      showNotice(String(e))
    }
  }, [showNotice])

  /** Update the replay control (play/pause, speed, seek). */
  const replayControl = useCallback(
    async (patch: { play?: boolean; speed?: number; seek_ms?: number; span_ms?: number }) => {
      const args: Record<string, unknown> = {}
      if (patch.play !== undefined) args.play = patch.play
      if (patch.speed !== undefined) args.speed = patch.speed
      if (patch.seek_ms !== undefined) args.seekMs = patch.seek_ms
      if (patch.span_ms !== undefined) args.spanMs = patch.span_ms
      try {
        setReplay(await invoke<ReplayStatus>('inspector_replay', args))
      } catch (e) {
        showNotice(String(e))
      }
    },
    [showNotice],
  )

  /** Stop replaying and hand the source back to the live link. */
  const useLive = useCallback(async () => {
    try {
      await invoke('inspector_use_live')
    } catch (e) {
      showNotice(String(e))
    }
    setReplay(null)
    setSeekPreview(null)
  }, [showNotice])

  const pausedRef = useRef(paused)
  pausedRef.current = paused
  const windowRef = useRef(windowSec)
  windowRef.current = windowSec
  const plotsRef = useRef(plots)
  plotsRef.current = plots
  const activePlotRef = useRef(activePlot)
  activePlotRef.current = activePlot

  const buffers = useRef<Map<string, Ring>>(new Map())
  /** Latest Rust-computed spectrum per trace (S3). */
  const spectra = useRef<Map<string, SpectrumFrame>>(new Map())
  const charts = useRef<Map<string, uPlot>>(new Map())
  const chartSigs = useRef<Map<string, string>>(new Map())
  /** Keeps each chart's canvas in sync with its grid cell (layout presets,
   * window resize); uPlot only sizes on construction otherwise. */
  const chartResizers = useRef<Map<string, ResizeObserver>>(new Map())
  const chartHosts = useRef<Map<string, HTMLDivElement>>(new Map())
  /** Per-plot DOM node the uPlot legend is mounted into, so it never overlays
   * the curve or axes (uPlot's default legend sits inside the plot box). */
  const chartLegends = useRef<Map<string, HTMLDivElement>>(new Map())
  /** Pinned cursor-A time (seconds) per plot, for the dual-cursor measurement. */
  const pins = useRef<Map<string, number | null>>(new Map())
  /** Per-plot DOM node the Δt/Δy readout is written into (no React churn). */
  const chartMeasures = useRef<Map<string, HTMLDivElement>>(new Map())
  const measureEnabled = useRef(false)
  measureEnabled.current = measure
  const followRef = useRef(follow)
  followRef.current = follow
  /** The x-range we last set per chart, to tell our auto-scale from a user zoom. */
  const expectedX = useRef<Map<uPlot, { min: number; max: number }>>(new Map())

  /** New samples set this; a requestAnimationFrame loop redraws (S4: no React
   * re-render per frame). */
  const dirty = useRef(false)
  const markDirty = useCallback(() => {
    dirty.current = true
  }, [])

  /** checked = the signals in the active plot (checking adds to it). */
  const checked = useMemo(
    () => new Set((plots.find((p) => p.id === activePlot)?.traces ?? []).map((t) => signalKey(t.signal))),
    [plots, activePlot],
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

  const removePlot = useCallback((id: string) => {
    setPlots((prev) => prev.filter((p) => p.id !== id))
    setActivePlot((cur) => (cur === id ? null : cur))
  }, [])

  const addPlot = useCallback(() => {
    const id = newId('p')
    setPlots((prev) => [...prev, { id, title: `Plot ${prev.length + 1}`, traces: [] }])
    setActivePlot(id)
  }, [])

  const addTraceToPlot = useCallback((plotId: string, signal: SignalId) => {
    setPlots((prev) =>
      prev.map((p) => (p.id === plotId ? { ...p, traces: [...p.traces, newTrace(signal)] } : p)),
    )
  }, [])

  /** Set the colour for a signal key (shared by all its traces/plots). */
  const setColor = useCallback((signalKey_: string, color: string) => {
    setColors((prev) => ({ ...prev, [signalKey_]: color }))
  }, [])

  /** Add or remove many signals on the active plot at once (group / search
   * select-all). Creates the active plot if none exists yet. */
  const toggleMany = useCallback((signals: SignalId[], add: boolean) => {
    const existing =
      activePlotRef.current != null && plotsRef.current.some((p) => p.id === activePlotRef.current)
    const pid = existing ? (activePlotRef.current as string) : newId('p')
    setPlots((prev) => {
      const base = existing ? prev : [...prev, { id: pid, title: 'Plot', traces: [] }]
      return base
        .map((p) => {
          if (p.id !== pid) return p
          let traces = p.traces
          for (const s of signals) {
            const key = signalKey(s)
            const has = traces.some((t) => signalKey(t.signal) === key)
            if (add && !has) traces = [...traces, newTrace(s)]
            else if (!add && has) traces = traces.filter((t) => signalKey(t.signal) !== key)
          }
          return { ...p, traces }
        })
        .filter((p) => p.traces.length > 0)
    })
    if (!existing) setActivePlot(pid)
  }, [])

  /** Check/uncheck a signal: add/remove a trace on the active plot. Checking
   * with no plot yet creates one (checkboxes are the only add path; no menu). */
  const toggleSignal = useCallback(
    (signal: SignalId) => {
      const key = signalKey(signal)
      const id = activePlotRef.current
      const target = id ? plotsRef.current.find((p) => p.id === id) : undefined
      if (target) {
        const present = target.traces.some((t) => signalKey(t.signal) === key)
        setPlots((prev) =>
          prev
            .map((p) =>
              p.id === target.id
                ? {
                    ...p,
                    traces: present
                      ? p.traces.filter((t) => signalKey(t.signal) !== key)
                      : [...p.traces, newTrace(signal)],
                  }
                : p,
            )
            .filter((p) => p.traces.length > 0),
        )
        return
      }
      const nid = newId('p')
      setPlots((prev) => [...prev, { id: nid, title: key, traces: [newTrace(signal)] }])
      setActivePlot(nid)
    },
    [],
  )

  // Workspace save/load (P7 + S2/S3): plots (with full SignalIds + chains),
  // colors, window and axis link. v1 (`checked`/`filterBy` by key) is dropped.
  const saveWorkspace = useCallback(() => {
    const ws: Workspace = { version: 2, windowSec, plots, colors, axisLink, layoutCols }
    localStorage.setItem(WS_KEY, JSON.stringify(ws))
  }, [windowSec, plots, colors, axisLink, layoutCols])

  const loadWorkspace = useCallback(() => {
    const raw = localStorage.getItem(WS_KEY)
    if (!raw) return
    try {
      const ws = JSON.parse(raw) as Workspace
      if (typeof ws.windowSec === 'number') setWindowSec(ws.windowSec)
      if (Array.isArray(ws.plots)) setPlots(ws.plots)
      if (ws.colors && typeof ws.colors === 'object') setColors(ws.colors)
      if (typeof ws.layoutCols === 'number') setLayoutCols(ws.layoutCols)
      setAxisLink(Boolean(ws.axisLink))
    } catch {
      // Corrupt workspace: keep the defaults.
    }
  }, [])

  /** Route one live sample to every trace bound to its signal. */
  const routeSample = useCallback(
    (id: SignalId, t_ms: number, raw: number, filtered: number) => {
      const key = signalKey(id)
      for (const p of plotsRef.current) {
        for (const t of p.traces) {
          if (signalKey(t.signal) !== key) continue
          let ring = buffers.current.get(t.id)
          if (!ring) {
            ring = new Ring(RING_CAP)
            buffers.current.set(t.id, ring)
          }
          ring.push(t_ms, raw, filtered)
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
        for (const s of mockSamples(now)) routeSample(s.id, s.t_ms, s.value, NaN)
        markDirty()
      }, 10)
      setCatalog(mockCatalog())
      setAlgorithms(mockAlgorithms())
      return () => clearInterval(id)
    }
    let disposed = false
    const channel = new Channel<FramePayload>()
    const onMessage = (frame: FramePayload) => {
      for (const tf of frame.traces) {
        let ring = buffers.current.get(tf.trace_id)
        if (!ring) {
          ring = new Ring(RING_CAP)
          buffers.current.set(tf.trace_id, ring)
        }
        for (let i = 0; i < tf.t.length; i++) {
          // serde_json writes NaN as null; turn it back into NaN (a gap in uPlot).
          ring.push(tf.t[i], tf.raw[i] ?? NaN, tf.filtered[i] ?? NaN)
        }
      }
      for (const sf of frame.spectra) spectra.current.set(sf.trace_id, sf.frame)
      markDirty()
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
  }, [tauri, markDirty, routeSample])

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
  // Throttled to `CATALOG_POLL_MS`; see the constant for why this is the right
  // throttling layer (the Rust rate EMA stays per-message).
  useEffect(() => {
    if (!tauri) return
    let lastMsgs = 0
    let lastT = 0
    const id = setInterval(async () => {
      try {
        const cat = await invoke<CatalogEntry[]>('inspector_catalog')
        const s = await invoke<InspectorStatus>('inspector_status')
        try {
          const rs = await invoke<ReplayStatus>('inspector_replay_status')
          setReplay(rs.active ? rs : null)
        } catch {
          /* replay status is optional */
        }
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
    }, CATALOG_POLL_MS)
    return () => clearInterval(id)
  }, [tauri])

  // Trim every ring to the selected window (keep at least one FFT window in
  // mock mode). Runs during the redraw loop so a shrinking window drops old
  // samples immediately; the ring cap bounds memory while paused.
  const trimRings = useCallback(() => {
    const keep = Math.max(tauri ? 2 : 1024, Math.ceil((windowRef.current * 1000) / 10))
    for (const ring of buffers.current.values()) {
      ring.trimBefore(ring.newestTime() - windowRef.current * 1000, keep)
    }
  }, [tauri])

  // A window change only needs a redraw; the loop trims to the new window.
  useEffect(() => {
    markDirty()
  }, [windowSec, markDirty])

  /** Fit one plot's x axis to its buffered data (and stop following). */
  const fitPlot = useCallback((plotId: string) => {
    const p = plotsRef.current.find((x) => x.id === plotId)
    const chart = charts.current.get(plotId)
    if (!p || !chart) return
    let min = Infinity
    let max = -Infinity
    for (const t of p.traces) {
      const r = buffers.current.get(t.id)
      if (!r || r.length === 0) continue
      min = Math.min(min, r.timeAt(0) / 1000)
      max = Math.max(max, r.newestTime() / 1000)
    }
    if (Number.isFinite(min) && max > min) chart.setScale('x', { min, max })
  }, [])

  const fitAll = useCallback(() => {
    setFollow(false)
    for (const id of charts.current.keys()) fitPlot(id)
  }, [fitPlot])

  // Create/destroy one uPlot per plot. Recreated when its trace set or the
  // per-trace colors change (series are fixed at construction) or axis linking
  // toggles (the cursor sync key has no runtime setter).
  useEffect(() => {
    for (const p of plots) {
      if (p.traces.length === 0) continue
      const host = chartHosts.current.get(p.id)
      if (!host) continue
      const sig = `${axisLink}|${p.traces
        .map(
          (t) =>
            `${t.id}:${colorOf(signalKey(t.signal), colors)}:${t.pipeline.length > 0 ? 1 : 0}`,
        )
        .join(',')}`
      if (charts.current.has(p.id) && chartSigs.current.get(p.id) === sig) continue
      charts.current.get(p.id)?.destroy()
      chartResizers.current.get(p.id)?.disconnect()
      const cursor = axisLink ? { show: true, sync: { key: SYNC_KEY } } : { show: true }
      const series: uPlot.Series[] = [{ label: 't', stroke: 'transparent' }]
      for (const t of p.traces) {
        const color = colorOf(signalKey(t.signal), colors)
        // Short label (field only) keeps the legend on one line.
        const name = t.signal.field
        if (t.pipeline.length === 0) {
          // Without a filter there is only one curve; no phantom "filtered"
          // series (its all-NaN data used to read back as "NaN" in the legend).
          series.push({ label: name, stroke: color, width: 2 })
        } else {
          // The filtered curve gets its own hue so it never hides behind raw.
          series.push({ label: `${name} raw`, stroke: color, width: 1, dash: [4, 3] })
          series.push({ label: `${name} filtered`, stroke: filteredColor(color), width: 2 })
        }
      }
      const empty = [
        [],
        ...p.traces.flatMap((t) => (t.pipeline.length > 0 ? [[], []] : [[]])),
      ] as unknown as uPlot.AlignedData
      const chart = new uPlot(
        {
          width: host.clientWidth,
          height: 220,
          legend: {
            show: true,
            live: true,
            mount: (_self, legendEl) => {
              chartLegends.current.get(p.id)?.replaceChildren(legendEl)
            },
          },
          cursor,
          plugins: [
            deltaCursorPlugin(p.id, pins, measureEnabled, () => chartMeasures.current.get(p.id)),
            panFitPlugin(p.id, fitPlot),
            {
              // A user drag-zoom (or pan) changes x away from the auto window:
              // stop following so the zoom sticks.
              hooks: {
                setScale: (u, key) => {
                  if (key !== 'x' || !followRef.current) return
                  const t = expectedX.current.get(u)
                  const s = u.scales.x
                  if (
                    t &&
                    (Math.abs((s.min ?? 0) - t.min) > 1e-6 ||
                      Math.abs((s.max ?? 0) - t.max) > 1e-6)
                  ) {
                    setFollow(false)
                  }
                },
              },
            },
          ],
          scales: { x: { time: false, auto: false }, y: { auto: true } },
          axes: [
            { stroke: INK(), grid: { stroke: MUTED(), width: 1, dash: [2, 4] } },
            { stroke: INK() },
          ],
          series,
        },
        empty,
        host,
      )
      charts.current.set(p.id, chart)
      chartSigs.current.set(p.id, sig)
      const ro = new ResizeObserver(() => {
        const w = host.clientWidth
        if (w > 0 && Math.abs(w - chart.width) > 1) chart.setSize({ width: w, height: 220 })
      })
      ro.observe(host)
      chartResizers.current.set(p.id, ro)
    }
    for (const id of [...charts.current.keys()]) {
      if (!plots.some((p) => p.id === id && p.traces.length > 0)) {
        charts.current.get(id)?.destroy()
        charts.current.delete(id)
        chartSigs.current.delete(id)
        chartResizers.current.get(id)?.disconnect()
        chartResizers.current.delete(id)
        pins.current.delete(id)
      }
    }
    markDirty()
  }, [plots, axisLink, colors, markDirty, fitPlot])

  // Redraw on a requestAnimationFrame loop instead of a React render per frame
  // (S4). Reads refs only; React state is structure, not data.
  const renderAll = useCallback(() => {
    trimRings()
    for (const p of plotsRef.current) {
      const chart = charts.current.get(p.id)
      if (!chart) continue
      const host = chartHosts.current.get(p.id)
      const maxPoints = Math.max(256, (host?.clientWidth ?? 800) * 2)
      const merged = mergedData(p, buffers.current, tauri, maxPoints)
      if (!merged) continue
      chart.setData(merged.data)
      if (followRef.current && merged.tEnd > 0) {
        const min = merged.tEnd - windowRef.current
        expectedX.current.set(chart, { min, max: merged.tEnd })
        chart.setScale('x', { min, max: merged.tEnd })
      }
    }
  }, [tauri, trimRings])

  useEffect(() => {
    let raf = 0
    const tick = () => {
      raf = requestAnimationFrame(tick)
      if (!dirty.current) return
      dirty.current = false
      // Paused = freeze the view (SDI behavior); buffering continues above.
      if (pausedRef.current) return
      renderAll()
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [renderAll])

  // Structural/scale changes ask for a redraw even without new samples.
  useEffect(() => {
    markDirty()
  }, [windowSec, plots, colors, paused, markDirty])

  // Turning the measurement off clears every pinned cursor and its readout.
  useEffect(() => {
    if (measure) return
    pins.current.clear()
    for (const chart of charts.current.values()) chart.redraw()
    for (const el of chartMeasures.current.values()) {
      el.replaceChildren()
      el.style.display = 'none'
    }
  }, [measure])

  // Keep the active plot (the checkbox target) pointing at a plot that exists.
  useEffect(() => {
    if (activePlot && plots.some((p) => p.id === activePlot)) return
    setActivePlot(plots[0]?.id ?? null)
  }, [plots, activePlot])

  useEffect(() => {
    for (const chart of charts.current.values()) chart.redraw()
  }, [windowSec])

  const clearAll = () => {
    for (const ring of buffers.current.values()) ring.clear()
    for (const chart of charts.current.values()) chart.setData(chart.series.map(() => []) as unknown as uPlot.AlignedData)
  }

  const exportCsv = () => {
    const rows: string[] = ['t_ms,trace,raw,filtered']
    for (const p of plots) {
      for (const t of p.traces) {
        const ring = buffers.current.get(t.id)
        if (!ring) continue
        for (let i = 0; i < ring.length; i++) {
          rows.push(`${ring.timeAt(i).toFixed(1)},${signalKey(t.signal)},${ring.rawAt(i)},${ring.filtAt(i)}`)
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
    const ring = buffers.current.get(trace.id)
    if (!ring) return null
    const n = 1024
    const recent: number[] = []
    for (let i = Math.max(0, ring.length - n); i < ring.length; i++) recent.push(ring.rawAt(i))
    if (recent.length < n) return null
    const fs = estimateFs(ring)
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
    (id: string, algorithm: string, params: [string, number][]) => {
      patchTrace(id, (t) => ({ ...t, pipeline: [...t.pipeline, { algorithm, params }] }))
    },
    [patchTrace],
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

  /**
   * Commit the seek slider: drop the buffered view so the plot jumps to the new
   * position (otherwise a backward seek leaves the old x-window on screen) and
   * force-follow, then ask the backend to stream that region.
   */
  const commitSeek = useCallback(() => {
    setSeekPreview((preview) => {
      if (preview != null) {
        buffers.current.clear()
        spectra.current.clear()
        setFollow(true)
        markDirty()
        void replayControl({ seek_ms: preview, span_ms: windowRef.current * 1000 })
      }
      return null
    })
  }, [replayControl, markDirty])


  return (
    <div className="inspector-grid">
      <div className="toolbar">
        <button
          className={`icon-btn ${paused ? 'active' : ''}`}
          title={paused ? 'Resume' : 'Pause'}
          aria-label={paused ? 'Resume' : 'Pause'}
          onClick={() => setPaused(!paused)}
        >
          {paused ? <Play size={16} /> : <Pause size={16} />}
        </button>
        <select value={windowSec} title="Time window" onChange={(e) => setWindowSec(Number(e.target.value))}>
          <option value={10}>10 s</option>
          <option value={30}>30 s</option>
          <option value={60}>60 s</option>
        </select>
        <select
          value={layoutCols}
          title="Plot grid layout"
          onChange={(e) => setLayoutCols(Number(e.target.value))}
        >
          <option value={1}>1×1</option>
          <option value={2}>2×1</option>
          <option value={3}>3×1</option>
        </select>
        <span className="toolbar-sep" />
        <button
          className={`icon-btn ${axisLink ? 'active' : ''}`}
          title="Link plot cursors"
          aria-label="Link plot cursors"
          onClick={() => setAxisLink(!axisLink)}
        >
          <Link2 size={16} />
        </button>
        <button
          className={`icon-btn ${measure ? 'active' : ''}`}
          title="Measure Δt / Δy (click a plot to pin)"
          aria-label="Measure cursors"
          onClick={() => setMeasure(!measure)}
        >
          <Ruler size={16} />
        </button>
        <button
          className={`icon-btn ${follow ? 'active' : ''}`}
          title="Follow the newest data (drag-zoom a plot to pause it)"
          aria-label="Follow newest data"
          onClick={() => {
            setFollow(true)
            markDirty()
          }}
        >
          <ChevronsRight size={16} />
        </button>
        <button className="icon-btn" title="Fit plot(s) to data" aria-label="Fit to data" onClick={fitAll}>
          <Maximize2 size={16} />
        </button>
        <span className="toolbar-sep" />
        <button className="icon-btn" title="New plot" aria-label="New plot" onClick={addPlot}>
          <Plus size={16} />
        </button>
        <button className="icon-btn" title="Clear data" aria-label="Clear data" onClick={clearAll}>
          <Eraser size={16} />
        </button>
        <div className="toolbar-menu" onMouseLeave={() => setMenuOpen(false)}>
          <button
            className={`icon-btn ${menuOpen ? 'active' : ''}`}
            title="More"
            aria-label="More"
            onClick={() => setMenuOpen((o) => !o)}
          >
            <MoreHorizontal size={16} />
          </button>
          {menuOpen && (
            <div className="toolbar-menu-pop">
              <button onClick={() => { exportCsv(); setMenuOpen(false) }}>Export CSV</button>
              <button onClick={() => { saveWorkspace(); setMenuOpen(false) }}>Save workspace</button>
              <button onClick={() => { loadWorkspace(); setMenuOpen(false) }}>Load workspace</button>
            </div>
          )}
        </div>
        {tauri && replay?.active && (
          <>
            <span className="toolbar-sep" />
            <button
              className="icon-btn"
              title={replay.playing ? 'Pause replay' : 'Play replay'}
              aria-label={replay.playing ? 'Pause replay' : 'Play replay'}
              onClick={() => void replayControl({ play: !replay.playing })}
            >
              {replay.playing ? <Pause size={16} /> : <Play size={16} />}
            </button>
            <span className="replay-name" title={replay.name ?? ''}>
              {replay.name}
            </span>
            <input
              className="replay-seek"
              type="range"
              min={0}
              max={Math.max(1, Math.round(replay.duration_ms))}
              step={1}
              value={Math.round(seekPreview ?? replay.position_ms)}
              title="Seek"
              aria-label="Seek replay"
              onChange={(e) => setSeekPreview(Number(e.target.value))}
              onMouseUp={commitSeek}
              onKeyUp={commitSeek}
            />
            <span className="replay-time">
              {fmtClock((seekPreview ?? replay.position_ms) / 1000)} / {fmtClock(replay.duration_ms / 1000)}
            </span>
            <select
              className="replay-speed"
              value={replay.speed}
              title="Replay speed"
              aria-label="Replay speed"
              onChange={(e) => void replayControl({ speed: Number(e.target.value) })}
            >
              {REPLAY_SPEEDS.map((sp) => (
                <option key={sp} value={sp}>
                  {sp}×
                </option>
              ))}
            </select>
            <button
              className="icon-btn"
              title="Back to live"
              aria-label="Back to live"
              onClick={() => void useLive()}
            >
              <Radio size={16} />
            </button>
          </>
        )}
        {tauri && !replay?.active && (
          <>
            <span className="toolbar-sep" />
            <button
              className="icon-btn"
              title="Open ULog for replay"
              aria-label="Open ULog"
              onClick={() => void openUlog()}
            >
              <FolderOpen size={16} />
            </button>
          </>
        )}
        <span style={{ marginLeft: 'auto', color: 'var(--mg-ink)', fontSize: 11 }}>
          {tauri && status && (
            <>
              {replay?.active ? (
                <>
                  <span style={{ color: 'var(--mg-ok)' }}>● replay</span>
                  {' · '}
                  {replay.name} · {(replay.position_ms / 1000).toFixed(1)} s /{' '}
                  {(replay.duration_ms / 1000).toFixed(1)} s
                </>
              ) : (
                <>
                  {status.connected ? <span style={{ color: 'var(--mg-ok)' }}>● link</span> : <span style={{ color: 'var(--mg-warn)' }}>● no link</span>}
                  {' · '}
                  {status.tap_running ? `${rate.toFixed(0)} msg/s` : '—'} · dropped {status.dropped}
                </>
              )}
            </>
          )}
          {'  '}Signal Inspector{tauri ? '' : ' · mock'}
        </span>
      </div>
      {notice && <div className="inspector-notice">{notice}</div>}

      <SignalBrowser
        catalog={catalog}
        checked={checked}
        colors={colors}
        onColor={setColor}
        onToggle={toggleSignal}
        onToggleMany={toggleMany}
      />

      <div className="plot-grid" style={{ gridTemplateColumns: `repeat(${layoutCols}, minmax(0, 1fr))` }}>
        {plots.length === 0 && (
          <div className="plot-empty">No plots yet — check a signal on the left to start plotting.</div>
        )}
        {plots.map((p) => (
          <div
            className={`plot ${activePlot === p.id ? 'active' : ''} ${dropPlot === p.id ? 'drop-target' : ''}`}
            key={p.id}
            onClick={() => setActivePlot(p.id)}
            onDragOver={(e) => {
              if (!e.dataTransfer.types.includes('text/plain')) return
              e.preventDefault()
              e.dataTransfer.dropEffect = 'copy'
              if (dropPlot !== p.id) setDropPlot(p.id)
            }}
            onDragLeave={() => setDropPlot((cur) => (cur === p.id ? null : cur))}
            onDrop={(e) => {
              e.preventDefault()
              setDropPlot(null)
              const entry = catalog.find((c) => signalKey(c.signal) === e.dataTransfer.getData('text/plain'))
              if (entry) addTraceToPlot(p.id, entry.signal)
            }}
          >
            <div className="plot-head">
              <PlotTitle title={p.title} onRename={(t) => setPlots((prev) => prev.map((x) => (x.id === p.id ? { ...x, title: t } : x)))} />
              <span className="hint">check signals at left to add</span>
              <button title="Remove plot" onClick={() => removePlot(p.id)}>
                ×
              </button>
            </div>
            {p.traces.length === 0 ? (
              <div className="peak-note">Empty plot — add a signal.</div>
            ) : (
              <>
                <div
                  ref={(el) => {
                    if (el) chartHosts.current.set(p.id, el)
                    else chartHosts.current.delete(p.id)
                  }}
                  className="chart"
                  title="Drag: zoom time · Shift+drag: pan · Double-click: fit"
                />
                <div
                  ref={(el) => {
                    if (el) chartLegends.current.set(p.id, el)
                    else chartLegends.current.delete(p.id)
                  }}
                  className="plot-legend"
                />
                <div
                  ref={(el) => {
                    if (el) chartMeasures.current.set(p.id, el)
                    else chartMeasures.current.delete(p.id)
                  }}
                  className="measure-readout"
                  style={{ display: 'none' }}
                />
              </>
            )}
            <div className="trace-chips">
              {p.traces.map((t) => (
                <span className={`trace-chip ${selected === t.id ? 'selected' : ''}`} key={t.id}>
                  <span
                    className="chip-color"
                    title="Raw color"
                    style={{ background: colorOf(signalKey(t.signal), colors) }}
                  />
                  {t.pipeline.length > 0 && (
                    <span
                      className="chip-color"
                      title="Filtered color"
                      style={{ background: filteredColor(colorOf(signalKey(t.signal), colors)) }}
                    />
                  )}
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
        color={selectedTrace ? colorOf(signalKey(selectedTrace.signal), colors) : undefined}
        catalog={catalog}
        algorithms={algorithms}
        buffer={selected ? buffers.current.get(selected) : undefined}
        tauri={tauri}
        onColor={(c) => selectedTrace && setColor(signalKey(selectedTrace.signal), c)}
        onRemove={() => selected && removeTrace(selected)}
        onAddStage={(algorithm, params) => selected && addStage(selected, algorithm, params)}
        onRemoveStage={(i) => selected && removeStage(selected, i)}
        onSetStage={(i, next) => selected && setStage(selected, i, next)}
        onError={showNotice}
      />
    </div>
  )
}

/** Left panel: catalog grouped by message with a search box and collapsible
 * groups. Grouping makes the tree usable with PX4's hundreds of fields. */
/** Inline-editable plot title: double-click to rename, Enter/blur commits. */
function PlotTitle({ title, onRename }: { title: string; onRename: (t: string) => void }) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(title)
  if (!editing) {
    return (
      <span
        className="title"
        title="Double-click to rename"
        onDoubleClick={(e) => {
          e.stopPropagation()
          setDraft(title)
          setEditing(true)
        }}
      >
        {title || 'Plot'}
      </span>
    )
  }
  const commit = () => {
    onRename(draft.trim() || title)
    setEditing(false)
  }
  return (
    <input
      className="title-edit"
      autoFocus
      value={draft}
      onClick={(e) => e.stopPropagation()}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === 'Enter') commit()
        else if (e.key === 'Escape') setEditing(false)
      }}
    />
  )
}

/** SDI-style per-signal colour picker: a swatch that opens a small palette. */
function ColorPicker({ color, onPick }: { color: string; onPick: (c: string) => void }) {
  const [open, setOpen] = useState(false)
  return (
    <span className="color-picker" onClick={(e) => e.preventDefault()}>
      <button
        type="button"
        className="color-swatch"
        style={{ background: color }}
        title="Signal color"
        onClick={(e) => {
          e.preventDefault()
          e.stopPropagation()
          setOpen((o) => !o)
        }}
      />
      {open && (
        <div className="color-pop">
          {PALETTE.map((c) => (
            <button
              type="button"
              key={c}
              className="color-dot"
              style={{ background: c }}
              title={c}
              onClick={(e) => {
                e.preventDefault()
                e.stopPropagation()
                onPick(c)
                setOpen(false)
              }}
            />
          ))}
          <input
            type="color"
            value={color}
            title="Custom color"
            onChange={(e) => {
              onPick(e.target.value)
              setOpen(false)
            }}
          />
        </div>
      )}
    </span>
  )
}

function SignalBrowser({
  catalog,
  checked,
  colors,
  onColor,
  onToggle,
  onToggleMany,
}: {
  catalog: CatalogEntry[]
  checked: Set<string>
  colors: Record<string, string>
  onColor: (key: string, color: string) => void
  onToggle: (signal: SignalId) => void
  onToggleMany: (signals: SignalId[], add: boolean) => void
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

  const shownCount = visible.reduce((n, g) => n + g.entries.length, 0)

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
      {q && shownCount > 0 && (
        <button className="add-shown" onClick={() => onToggleMany(visible.flatMap((g) => g.entries.map((c) => c.signal)), true)}>
          + Add {shownCount} shown
        </button>
      )}
      {catalog.length === 0 && <div style={{ color: 'var(--mg-ink)' }}>Waiting for data…</div>}
      {visible.map((g) => {
        const isCollapsed = collapsed.has(g.name)
      const allInGroup = g.entries.every((c) => checked.has(signalKey(c.signal)))
      const someInGroup = g.entries.some((c) => checked.has(signalKey(c.signal)))
      return (
        <div className="signal-group" key={g.name}>
          <div className="signal-group-head" onClick={() => toggleGroup(g.name)}>
            <input
              type="checkbox"
              className="group-check"
              checked={allInGroup}
              title="Add / remove every signal in this group"
              ref={(el) => {
                if (el) el.indeterminate = someInGroup && !allInGroup
              }}
              onClick={(e) => e.stopPropagation()}
              onChange={() => onToggleMany(g.entries.map((c) => c.signal), !allInGroup)}
            />
            <span className={isCollapsed ? 'caret caret-closed' : 'caret'} />
              <span>{g.name}</span>
              <span className="msg">
                {g.entries.length}
              </span>
            </div>
            {!isCollapsed &&
              g.entries.map((c) => {
                const key = signalKey(c.signal)
                return (
                  <label
                    key={key}
                    draggable
                    title="Drag onto a plot to add it"
                    onDragStart={(e) => {
                      e.dataTransfer.setData('text/plain', key)
                      e.dataTransfer.effectAllowed = 'copy'
                    }}
                  >
                    <input type="checkbox" checked={checked.has(key)} onChange={() => onToggle(c.signal)} />
                    <ColorPicker color={colorOf(key, colors)} onPick={(col) => onColor(key, col)} />
                    <span className="mono">{c.signal.field}</span>
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
 * series to share x), padding each series with NaN where it has no sample.
 * When the union axis exceeds `maxPoints`, min/max decimation (S4) caps the
 * number of columns drawn without hiding peaks. */
function mergedData(
  plot: Plot,
  buffers: Map<string, Ring>,
  tauri: boolean,
  maxPoints: number,
): { data: uPlot.AlignedData; tEnd: number } | null {
  const traces = plot.traces
  const k = traces.length
  const rings = traces.map((tr) => buffers.get(tr.id))
  // Mock mode has no Rust analyzer, so precompute the TS mirror of the
  // filtered line once and read it by index during the merge.
  const mirrors: number[][] | null = tauri
    ? null
    : traces.map((tr, i) => {
        const r = rings[i]
        if (!r || r.length === 0) return []
        const raw: number[] = []
        for (let j = 0; j < r.length; j++) raw.push(r.rawAt(j))
        return mirrorFilter(raw, tr.pipeline, estimateFs(r))
      })

  // k-way merge over the (already sorted) per-trace rings.
  const cursor = new Array<number>(k).fill(0)
  const xs: number[] = []
  const rawCols: number[][] = traces.map(() => [])
  const filtCols: number[][] = traces.map(() => [])
  for (;;) {
    let minT = Infinity
    for (let i = 0; i < k; i++) {
      const r = rings[i]
      if (r && cursor[i] < r.length) {
        const t = r.timeAt(cursor[i])
        if (t < minT) minT = t
      }
    }
    if (!Number.isFinite(minT)) break
    xs.push(minT)
    for (let i = 0; i < k; i++) {
      const r = rings[i]
      const j = cursor[i]
      if (r && j < r.length && r.timeAt(j) === minT) {
        rawCols[i].push(r.rawAt(j))
        filtCols[i].push(tauri ? r.filtAt(j) : mirrors![i][j])
        cursor[i] = j + 1
      } else {
        rawCols[i].push(NaN)
        filtCols[i].push(NaN)
      }
    }
  }

  let outX = xs
  let outRaw = rawCols
  let outFilt = filtCols
  if (xs.length > maxPoints) {
    const d = decimateMinMax(xs, rawCols, filtCols, maxPoints)
    outX = d.x
    outRaw = d.raw
    outFilt = d.filt
  }
  if (outX.length < 2) return null

  const x = Float64Array.from(outX, (v) => v / 1000)
  // uPlot expresses gaps with `null`/`undefined`, never `NaN`: its y-range scan
  // keeps `NaN` (only `null` is skipped), so one `NaN` at the edge of the view
  // poisons the whole scale and blanks every curve in the plot.
  const gap = (v: number): number | null => (Number.isFinite(v) ? v : null)
  const series: (Float64Array | (number | null)[])[] = [x]
  for (let i = 0; i < k; i++) {
    series.push(outRaw[i].map(gap))
    // Only filtered traces carry a second column, matching the series list.
    if (traces[i].pipeline.length > 0) series.push(outFilt[i].map(gap))
  }
  // `tEnd` is in seconds to match the x data (uPlot scales use the data units).
  return { data: series as uPlot.AlignedData, tEnd: outX[outX.length - 1] / 1000 }
}

/** Min/max decimation over a shared time axis: each bucket keeps up to two
 * columns (the earliest and latest extreme of every series) so peaks survive
 * downsampling while all series keep the same x. */
function decimateMinMax(
  xs: number[],
  raw: number[][],
  filt: number[][],
  maxPoints: number,
): { x: number[]; raw: number[][]; filt: number[][] } {
  const n = xs.length
  const k = raw.length
  const buckets = Math.max(1, Math.floor(Math.max(2, maxPoints) / 2))
  const size = n / buckets
  const outX: number[] = []
  const outRaw: number[][] = raw.map(() => [])
  const outFilt: number[][] = filt.map(() => [])
  for (let b = 0; b < buckets; b++) {
    const lo = Math.floor(b * size)
    const hi = Math.min(n, Math.floor((b + 1) * size))
    if (hi <= lo) continue
    const x1 = xs[lo]
    const x2 = xs[hi - 1]
    const two = x2 > x1
    outX.push(x1)
    if (two) outX.push(x2)
    for (let i = 0; i < k; i++) {
      const col = raw[i]
      const fcol = filt[i]
      let minJ = -1
      let maxJ = -1
      for (let j = lo; j < hi; j++) {
        const v = col[j]
        if (v !== v) continue
        if (minJ < 0 || v < col[minJ]) minJ = j
        if (maxJ < 0 || v > col[maxJ]) maxJ = j
      }
      if (minJ < 0) {
        outRaw[i].push(NaN)
        outFilt[i].push(NaN)
        if (two) {
          outRaw[i].push(NaN)
          outFilt[i].push(NaN)
        }
        continue
      }
      const firstJ = minJ <= maxJ ? minJ : maxJ
      const lastJ = minJ <= maxJ ? maxJ : minJ
      outRaw[i].push(col[firstJ])
      outFilt[i].push(fcol[firstJ])
      if (two) {
        outRaw[i].push(col[lastJ])
        outFilt[i].push(fcol[lastJ])
      }
    }
  }
  return { x: outX, raw: outRaw, filt: outFilt }
}

/** Number formatter that tolerates null/NaN (serde_json turns NaN into null). */
function fmt(v: number | null | undefined, digits: number): string {
  return typeof v === 'number' && Number.isFinite(v) ? v.toFixed(digits) : '—'
}

function estimateFs(ring: Ring): number {
  const n = ring.length
  if (n < 2) return 100
  const dt = (ring.timeAt(n - 1) - ring.timeAt(0)) / (n - 1)
  return dt > 0 ? 1000 / dt : 100
}

/** Shift+drag pans the time axis; double-click fits the plot to its data. Both
 * gestures stop the x axis from following (the toolbar `Follow` restores it). */
function panFitPlugin(plotId: string, fit: (plotId: string) => void): uPlot.Plugin {
  return {
    hooks: {
      ready: (u) => {
        u.root.addEventListener('mousedown', (e) => {
          if (!e.shiftKey || e.button !== 0) return
          e.preventDefault()
          const startX = e.clientX
          const s = u.scales.x
          const startMin = s.min ?? 0
          const startMax = s.max ?? startMin + 1
          const perPx = (startMax - startMin) / Math.max(1, u.bbox.width)
          const move = (ev: MouseEvent) => {
            const d = -(ev.clientX - startX) * perPx
            u.setScale('x', { min: startMin + d, max: startMax + d })
          }
          const up = () => {
            window.removeEventListener('mousemove', move)
            window.removeEventListener('mouseup', up)
          }
          window.addEventListener('mousemove', move)
          window.addEventListener('mouseup', up)
        })
        u.root.addEventListener('dblclick', () => fit(plotId), true)
      },
    },
  }
}

/** SDI-style dual cursor (P7): with the Δ toggle on, clicking a plot pins
 * cursor A; the live cursor is B. Draws the pinned line and prints Δt plus each
 * series' Δy into a dedicated DOM node (outside React, so hovering does not
 * re-render the app). Clicking the same spot again clears the pin. */
function deltaCursorPlugin(
  plotId: string,
  pins: { current: Map<string, number | null> },
  enabled: { current: boolean },
  host: () => HTMLDivElement | undefined,
): uPlot.Plugin {
  const num = (v: number): string => (Number.isFinite(v) ? v.toPrecision(4) : '—')
  const valueAt = (u: uPlot, seriesIdx: number, x: number): number => {
    const arr = u.data[seriesIdx] as Array<number | null | undefined>
    if (!arr || arr.length === 0) return NaN
    const v = arr[u.valToIdx(x)]
    return typeof v === 'number' ? v : NaN
  }
  const render = (u: uPlot) => {
    const el = host()
    if (!el) return
    const pin = pins.current.get(plotId)
    if (!enabled.current || pin == null) {
      el.replaceChildren()
      el.style.display = 'none'
      return
    }
    const left = u.cursor.left
    const xB = left == null || left < 0 ? pin : u.posToVal(left, 'x')
    const rows: HTMLElement[] = []
    const dt = document.createElement('span')
    dt.className = 'm-dt'
    dt.textContent = `Δt ${num(xB - pin)} s`
    rows.push(dt)
    for (let i = 1; i < u.series.length; i++) {
      const yA = valueAt(u, i, pin)
      const yB = valueAt(u, i, xB)
      if (!Number.isFinite(yA) || !Number.isFinite(yB)) continue
      const item = document.createElement('span')
      item.className = 'm-item'
      item.textContent = `${u.series[i].label ?? `s${i}`}  Δy ${num(yB - yA)}`
      rows.push(item)
    }
    el.replaceChildren(...rows)
    el.style.display = 'flex'
  }
  const drawPin = (u: uPlot) => {
    const pin = pins.current.get(plotId)
    if (!enabled.current || pin == null) return
    const x = u.valToPos(pin, 'x')
    if (!Number.isFinite(x)) return
    const { ctx } = u
    ctx.save()
    ctx.strokeStyle = MEASURE_COLOR
    ctx.lineWidth = 1
    ctx.setLineDash([4, 3])
    ctx.beginPath()
    ctx.moveTo(x, u.bbox.top)
    ctx.lineTo(x, u.bbox.top + u.bbox.height)
    ctx.stroke()
    ctx.restore()
  }
  return {
    hooks: {
      ready: (u) => {
        // Clicks land on the `.u-under` layer (a sibling of `.u-over`), so
        // listen on the root and hit-test against the plot bbox.
        // Capture phase: uPlot's own cursor `click` handler stops propagation,
        // so we must run before it.
        u.root.addEventListener(
          'click',
          (e) => {
            if (!enabled.current) return
            const r = u.root.getBoundingClientRect()
            const px = e.clientX - r.left - u.bbox.left
            const py = e.clientY - r.top - u.bbox.top
            if (px < 0 || px > u.bbox.width || py < 0 || py > u.bbox.height) return
            const x = u.posToVal(px, 'x')
            const cur = pins.current.get(plotId)
            pins.current.set(plotId, cur != null && Math.abs(cur - x) < 1e-9 ? null : x)
            u.redraw(false)
            render(u)
          },
          true,
        )
      },
      setCursor: (u) => render(u),
      draw: drawPin,
    },
  }
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
  buffer: Ring | undefined
  tauri: boolean
  onColor: (color: string) => void
  onRemove: () => void
  onAddStage: (algorithm: string, params: [string, number][]) => void
  onRemoveStage: (index: number) => void
  onSetStage: (index: number, next: AlgoConfig) => void
  onError: (msg: string) => void
}) {
  if (!selected || !trace) {
    return (
      <aside className="properties">
        <div className="prop-title">Properties</div>
        <div style={{ color: 'var(--mg-ink)', fontSize: 12 }}>No trace selected — click a trace chip.</div>
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
        <dt>Value</dt>
        <dd>{entry ? fmt(entry.last_value, 3) : '—'}</dd>
        <dt>Rate</dt>
        <dd>{entry ? `${fmt(entry.rate_hz, 1)} Hz` : '—'}</dd>
        <dt>Est. fs</dt>
        <dd>{fs > 0 ? `${fs.toFixed(1)} Hz` : '—'}</dd>
        <dt>Samples</dt>
        <dd>{buffer ? buffer.length : 0}</dd>
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
      <button
        onClick={() => {
          const a = algorithms.find((x) => x.kind === 'processor')
          if (!a) return
          onAddStage(a.id, defaultStageParams(a, fs > 0 ? fs : entry?.rate_hz ?? 0))
        }}
      >
        + Add filter stage
      </button>
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
  const roRef = useRef<ResizeObserver | null>(null)
  const [ready, setReady] = useState(false)

  const setHost = useCallback((el: HTMLDivElement | null) => {
    hostRef.current = el
    if (el) {
      if (!plotRef.current) {
        plotRef.current = new uPlot(
          {
            width: el.clientWidth || 640,
            height: 220,
            legend: { show: false },
            scales: { x: { time: false }, y: { auto: true } },
            axes: [
              {
                label: 'Hz',
                font: AXIS_FONT,
                labelFont: AXIS_FONT,
                labelSize: 16,
                size: 40,
                stroke: INK(),
                grid: { stroke: MUTED(), width: 1, dash: [2, 4] },
              },
              {
                label: 'magnitude',
                font: AXIS_FONT,
                labelFont: AXIS_FONT,
                labelSize: 16,
                size: 58,
                stroke: INK(),
                values: (_u, vals) => vals.map(fmtMag),
              },
            ],
            series: [
              { label: 'Hz', stroke: 'transparent' },
              { label: 'mag', stroke: token('--mg-mag'), width: 1 },
            ],
          },
          [[], []],
          el,
        )
        setReady(true)
      }
      roRef.current?.disconnect()
      const ro = new ResizeObserver(() => {
        const plot = plotRef.current
        const w = el.clientWidth
        if (plot && w > 0 && Math.abs(w - plot.width) > 1) plot.setSize({ width: w, height: 220 })
      })
      ro.observe(el)
      roRef.current = ro
    } else {
      roRef.current?.disconnect()
      roRef.current = null
      plotRef.current?.destroy()
      plotRef.current = null
      setReady(false)
    }
  }, [])

  useEffect(() => () => {
    roRef.current?.disconnect()
    roRef.current = null
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
    <div style={{ fontSize: 11, color: 'var(--mg-ink)', marginTop: 2 }}>
      {label} · peak {frame.peak_freq_hz.toFixed(1)} Hz · fs {frame.fs.toFixed(0)} Hz · Nyquist {frame.nyquist.toFixed(0)} Hz · Δf {frame.delta_f.toFixed(2)} Hz
      <div ref={setHost} className="chart" />
      {!ready && <div className="peak-note">allocating chart…</div>}
    </div>
  )
}
