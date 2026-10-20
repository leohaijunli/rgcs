// Live curves for the motor-test window (A5, plan §6.2): calibrated total
// field, per-ESC current and RPM over a rolling window, fed by the
// `actuator_telemetry` events the shell's tap emits (~10 Hz, latest snapshot
// semantics — one aligned row per update, nothing queued).
//
// The baseline button stores the current total field; the mag series then
// shows Δm = m − baseline (the interference the test is measuring).

import { useEffect, useRef, useState } from 'react'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'
import { listen } from '@tauri-apps/api/event'
import { cssVar } from '../design-system/theme'
import { isTauri } from '../inspector/mock'

/** Rolling window kept in the buffer, seconds. */
const WINDOW_S = 60
/** UI update cadence — the tap pushes at ~10 Hz; redraw at most this fast. */
const REDRAW_MS = 100

/** One aligned snapshot row. */
interface Row {
  t: number // seconds, epoch
  mag: number | null
  currents: (number | null)[]
  rpms: (number | null)[]
}

function emptyRow(): Row {
  return { t: 0, mag: null, currents: [null, null, null, null], rpms: [null, null, null, null] }
}

export default function LiveCharts() {
  const hostRef = useRef<HTMLDivElement | null>(null)
  const plotRef = useRef<uPlot | null>(null)
  const rowsRef = useRef<Row[]>([])
  const baselineRef = useRef<number | null>(null)
  /** Baseline UI state mirrors the ref (button label). */
  const [hasBaseline, setHasBaseline] = useState(false)

  useEffect(() => {
    if (!isTauri()) return
    let disposed = false
    let un: (() => void) | undefined

    const plot = new uPlot(
      {
        width: hostRef.current?.clientWidth || 600,
        height: hostRef.current?.clientHeight || 320,
        legend: { show: true, live: true },
        scales: { x: { time: false, auto: false }, y: { auto: true } },
        axes: [
          { stroke: cssVar('--mg-muted'), grid: { stroke: cssVar('--mg-border') } },
          {
            stroke: cssVar('--mg-muted'),
            label: 'nT / A',
            grid: { stroke: cssVar('--mg-border') },
          },
          {
            // RPM on the right axis.
            scale: 'rpm',
            side: 1,
            stroke: cssVar('--mg-muted'),
            grid: { show: false },
            label: 'RPM',
          },
        ],
        series: [
          {},
          {
            label: 'mag (nT)',
            stroke: cssVar('--mg-trace-1'),
            width: 2,
          },
          ...Array.from({ length: 4 }, (_, i) => ({
            label: `esc${i + 1} I`,
            stroke: cssVar(`--mg-trace-${i + 2}`),
            width: 1.5,
          })),
          ...Array.from({ length: 4 }, (_, i) => ({
            label: `esc${i + 1} RPM`,
            stroke: cssVar(`--mg-trace-${i + 2}`),
            width: 1,
            dash: [4, 3],
            scale: 'rpm',
          })),
        ],
      },
      [[]],
      hostRef.current as HTMLElement,
    )
    plotRef.current = plot

    const onTelemetry = (payload: {
      mag_total_nt: number | null
      esc: Array<[string, number]>
    }) => {
      const rows = rowsRef.current
      const now = performance.now() / 1000
      // Reuse the last row within one tick (the events land in pairs).
      const row = rows.length > 0 && now - rows[rows.length - 1].t < 0.05 ? rows[rows.length - 1] : null
      const target = row ?? emptyRow()
      if (!row) target.t = now
      if (payload.mag_total_nt != null) target.mag = payload.mag_total_nt
      for (const [name, value] of payload.esc) {
        const m = /^(esc\d)\.(rpm|current|voltage)$/.exec(name)
        if (!m) continue
        const idx = Number(m[1].slice(3)) - 1
        if (idx < 0 || idx > 3) continue
        if (m[2] === 'current') target.currents[idx] = value
        if (m[2] === 'rpm') target.rpms[idx] = value
      }
      if (!row) {
        rows.push(target)
        // Trim the rolling window.
        const cutoff = now - WINDOW_S
        while (rows.length > 0 && rows[0].t < cutoff) rows.shift()
      }
    }

    void (async () => {
      try {
        un = await listen<{
          mag_total_nt: number | null
          esc: Array<[string, number]>
        }>('actuator_telemetry', (e) => {
          if (disposed) return
          onTelemetry(e.payload)
        })
      } catch {
        /* events unavailable (browser dev) */
      }
    })()

    // Redraw on a cadence, not per event.
    const id = window.setInterval(() => {
      const p = plotRef.current
      if (!p) return
      const rows = rowsRef.current
      if (rows.length < 2) return
      const t0 = rows[0].t
      const col = (get: (r: Row) => number | null) =>
        rows.map((r) => {
          const v = get(r)
          return v == null ? null : v
        })
      const base = baselineRef.current
      const xs = rows.map((r) => r.t - t0)
      const series: (number | null)[][] = [
        col((r) => r.mag),
        ...Array.from({ length: 4 }, (_, i) => col((r) => r.currents[i])),
        ...Array.from({ length: 4 }, (_, i) => col((r) => r.rpms[i])),
      ]
      // Baseline: the mag series shows Δm once a baseline exists.
      if (base != null) {
        series[0] = series[0].map((v) => (v == null ? null : v - base))
      }
      p.setData([Float64Array.from(xs), ...series] as unknown as uPlot.AlignedData, true)
    }, REDRAW_MS)

    return () => {
      disposed = true
      un?.()
      window.clearInterval(id)
      plot.destroy()
      plotRef.current = null
    }
  }, [])

  const setBaseline = () => {
    const rows = rowsRef.current
    const last = rows.length > 0 ? rows[rows.length - 1].mag : null
    if (last == null) return
    baselineRef.current = last
    setHasBaseline(true)
  }

  const clearBaseline = () => {
    baselineRef.current = null
    setHasBaseline(false)
  }

  return (
    <div className="flex h-full flex-col gap-2">
      <div className="flex items-center gap-2">
        <span className="section-title">Live curves</span>
        {hasBaseline ? (
          <button className="mode-btn" style={{ padding: '2px 10px' }} onClick={clearBaseline}>
            Clear baseline
          </button>
        ) : (
          <button className="mode-btn" style={{ padding: '2px 10px' }} onClick={setBaseline}>
            Set baseline (Δmag from here)
          </button>
        )}
      </div>
      <div ref={hostRef} className="min-h-0 flex-1" />
    </div>
  )
}