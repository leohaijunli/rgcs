// Live servo curves for the motor-test window: the SITL servo commands
// (SERVO_OUTPUT_RAW input, ~25 Hz stream) against the PX4's decoded outputs
// (SERVO_OUTPUT_RAW from the serial link), one selectable strip per motor —
// plus the magnetometer total field before and after calibration. Fed by the
// `actuator_servo` and `actuator_telemetry` events the shell emits.

import { useEffect, useRef, useState } from 'react'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'
import { listen } from '@tauri-apps/api/event'
import { cssVar } from '../design-system/theme'
import { isTauri } from '../inspector/mock'

/** Rolling window kept in the buffers, seconds. */
const WINDOW_S = 60
/** Redraw cadence — the servo stream is ~25 Hz; redraw at most this fast. */
const REDRAW_MS = 80
const SERVO_HEIGHT = 190
const MAG_HEIGHT = 150

/** One servo sample row: per-motor normalized 0..1, SITL input + PX4 output. */
interface ServoRow {
  t: number
  sitl: number[]
  px4: number[]
}

/** One aligned mag row: total field before (raw) and after (calibrated). */
interface MagRow {
  t: number
  raw: number | null
  cal: number | null
}

export default function ServoCharts() {
  const servoHostRef = useRef<HTMLDivElement | null>(null)
  const magHostRef = useRef<HTMLDivElement | null>(null)
  const servoPlotRef = useRef<uPlot | null>(null)
  const magPlotRef = useRef<uPlot | null>(null)
  const servoRowsRef = useRef<ServoRow[]>([])
  const magRowsRef = useRef<MagRow[]>([])
  /** Per-channel visibility: M1..M4 (selectable strips). */
  const [channels, setChannels] = useState<boolean[]>([true, true, true, true])

  useEffect(() => {
    if (!isTauri()) return
    let disposed = false
    let unServo: (() => void) | undefined
    let unTelemetry: (() => void) | undefined

    // ---- Servo plot: 4 channels × (SITL input + PX4 output). ----
    const servo = new uPlot(
      {
        width: servoHostRef.current?.clientWidth || 600,
        height: SERVO_HEIGHT,
        legend: { show: true, live: true },
        scales: { x: { time: false, auto: false }, y: { min: 0, max: 1 } },
        axes: [
          { stroke: cssVar('--mg-muted'), grid: { stroke: cssVar('--mg-border') }, label: 's' },
          { stroke: cssVar('--mg-muted'), label: 'cmd 0..1', grid: { stroke: cssVar('--mg-border') } },
        ],
        series: [
          {},
          // M1 SITL / M1 PX4, ... — toggling a channel hides both its curves.
          ...Array.from({ length: 4 }, (_, i) => [
            {
              label: `M${i + 1} SITL`,
              stroke: cssVar('--mg-warn'),
              width: 2,
            },
            {
              label: `M${i + 1} PX4`,
              stroke: cssVar('--mg-ok'),
              width: 2,
            },
          ]).flat(),
        ],
      },
      [[]],
      servoHostRef.current as HTMLElement,
    )
    servoPlotRef.current = servo

    // ---- Mag plot: raw (before calibration) vs calibrated total. ----
    const mag = new uPlot(
      {
        width: magHostRef.current?.clientWidth || 600,
        height: MAG_HEIGHT,
        legend: { show: true, live: true },
        scales: { x: { time: false, auto: false }, y: { auto: true } },
        axes: [
          { stroke: cssVar('--mg-muted'), grid: { stroke: cssVar('--mg-border') }, label: 's' },
          { stroke: cssVar('--mg-muted'), label: 'nT', grid: { stroke: cssVar('--mg-border') } },
        ],
        series: [
          {},
          { label: 'mag before cal', stroke: cssVar('--mg-trace-2'), width: 1.5 },
          { label: 'mag after cal', stroke: cssVar('--mg-trace-1'), width: 2 },
        ],
      },
      [[]],
      magHostRef.current as HTMLElement,
    )
    magPlotRef.current = mag

    const onServo = (payload: { t: number; sitl: number[]; px4: number[] }) => {
      const rows = servoRowsRef.current
      rows.push({
        t: payload.t,
        sitl: Array.from({ length: 4 }, (_, i) => payload.sitl[i] ?? 0),
        px4: Array.from({ length: 4 }, (_, i) => payload.px4[i] ?? 0),
      })
      const cutoff = payload.t - WINDOW_S
      while (rows.length > 0 && rows[0].t < cutoff) rows.shift()
    }

    const onTelemetry = (payload: { mag_raw_total_nt: number | null; mag_total_nt: number | null }) => {
      const rows = magRowsRef.current
      const t = Date.now() / 1000
      rows.push({ t, raw: payload.mag_raw_total_nt, cal: payload.mag_total_nt })
      const cutoff = t - WINDOW_S
      while (rows.length > 0 && rows[0].t < cutoff) rows.shift()
    }

    void (async () => {
      try {
        unServo = await listen<{ t: number; sitl: number[]; px4: number[] }>(
          'actuator_servo',
          (e) => {
            if (disposed) return
            onServo(e.payload)
          },
        )
      } catch {
        /* events unavailable (browser dev) */
      }
    })()
    void (async () => {
      try {
        unTelemetry = await listen<{
          mag_raw_total_nt: number | null
          mag_total_nt: number | null
        }>('actuator_telemetry', (e) => {
          if (disposed) return
          onTelemetry(e.payload)
        })
      } catch {
        /* events unavailable (browser dev) */
      }
    })()

    const id = window.setInterval(() => {
      const t0 = Date.now() / 1000 - WINDOW_S
      // Servo plot: one aligned x per row, both curves per channel.
      const sRows = servoRowsRef.current
      if (sRows.length >= 2) {
        const xs = sRows.map((r) => r.t)
        const series: (number | null)[][] = Array.from({ length: 8 }, () => [])
        sRows.forEach((r) => {
          for (let i = 0; i < 4; i++) {
            series[i * 2].push(r.sitl[i])
            series[i * 2 + 1].push(r.px4[i])
          }
        })
        servoPlotRef.current?.setData(
          [xs, ...series] as unknown as uPlot.AlignedData,
          true,
        )
      }
      // Mag plot: raw + calibrated totals.
      const mRows = magRowsRef.current
      if (mRows.length >= 2) {
        const xs = mRows.map((r) => r.t)
        magPlotRef.current?.setData(
          [
            xs,
            mRows.map((r) => r.raw),
            mRows.map((r) => r.cal),
          ] as unknown as uPlot.AlignedData,
          true,
        )
      }
      void t0
    }, REDRAW_MS)

    return () => {
      disposed = true
      unServo?.()
      unTelemetry?.()
      window.clearInterval(id)
      servo.destroy()
      mag.destroy()
      servoPlotRef.current = null
      magPlotRef.current = null
    }
  }, [])

  // Toggling a channel hides (or shows) both its curves in the servo plot.
  const toggleChannel = (i: number) => {
    setChannels((prev) => {
      const next = prev.map((v, k) => (k === i ? !v : v))
      const p = servoPlotRef.current
      if (p) {
        p.setSeries(1 + i * 2, { show: next[i] })
        p.setSeries(1 + i * 2 + 1, { show: next[i] })
      }
      return next
    })
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <span className="section-title">Servo commands</span>
        {channels.map((on, i) => (
          <button
            key={i}
            className={`mode-btn chan-toggle ${on ? 'active' : ''}`}
            style={{ flex: 'none', padding: '2px 10px' }}
            onClick={() => toggleChannel(i)}
          >
            M{i + 1} {on ? '●' : '○'}
          </button>
        ))}
        <span className="estop-note" style={{ marginLeft: 'auto' }}>
          SITL input (amber) → PX4 output (green), decoded from serial
        </span>
      </div>
      <div ref={servoHostRef} style={{ height: SERVO_HEIGHT }} />

      <div className="flex items-center gap-2">
        <span className="section-title">Magnetometer total field</span>
        <span className="estop-note">before (dashed) and after calibration</span>
      </div>
      <div ref={magHostRef} style={{ height: MAG_HEIGHT }} />
    </div>
  )
}