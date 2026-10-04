import { useEffect, useRef } from 'react'
import { cssVar } from '../../design-system/theme'
import { useUiStore } from '../../stores/ui'

interface Props {
  label: string
  value: number
  min: number
  max: number
  /** Value per pixel (zoom of the tape). */
  perPx: number
  width?: number
  height?: number
  /** Warn/err thresholds relative to `value` for tick coloring (optional). */
  warnZone?: [number, number]
}

/** Vertical instrument tape (airspeed / altitude) with a fixed readout window. */
export default function VerticalTape({
  label,
  value,
  min,
  max,
  perPx,
  width = 64,
  height = 220,
  warnZone,
}: Props) {
  const ref = useRef<HTMLCanvasElement>(null)
  const theme = useUiStore((s) => s.theme)

  useEffect(() => {
    const canvas = ref.current
    if (!canvas) return
    const dpr = window.devicePixelRatio || 1
    canvas.width = width * dpr
    canvas.height = height * dpr
    const ctx = canvas.getContext('2d')
    if (!ctx) return
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)

    const bg = cssVar('--mg-instrument')

    const muted = cssVar('--mg-muted')
    const accent = cssVar('--mg-accent')

    const err = cssVar('--mg-error')
    const cy = height / 2

    ctx.fillStyle = bg
    ctx.fillRect(0, 0, width, height)

    // Tape ticks (only the on-screen range).
    for (let v = Math.max(min, Math.floor(value - (height / 2 + 8) / perPx)); v <= Math.min(max, Math.ceil(value + (height / 2 + 8) / perPx)); v += 1) {
      const y = cy + (value - v) * perPx
      const major = v % 5 === 0
      let color = muted
      if (warnZone) {
        if (v < warnZone[0] || v > warnZone[1]) color = err
        else if (Math.abs(v - value) < 15) color = accent
      }
      ctx.strokeStyle = color
      ctx.lineWidth = major ? 2 : 1
      const x0 = major ? width * 0.45 : width * 0.62
      ctx.beginPath()
      ctx.moveTo(x0, y)
      ctx.lineTo(width, y)
      ctx.stroke()
      if (major) {
        ctx.fillStyle = color
        ctx.font = '11px sans-serif'
        ctx.textAlign = 'left'
        ctx.fillText(String(v), 4, y + 4)
      }
    }

    // Readout window.
    ctx.strokeStyle = accent
    ctx.lineWidth = 2
    ctx.beginPath()
    ctx.moveTo(0, cy - 14)
    ctx.lineTo(width, cy - 14)
    ctx.moveTo(0, cy + 14)
    ctx.lineTo(width, cy + 14)
    ctx.stroke()
    ctx.fillStyle = bg
    ctx.fillRect(width - 44, cy - 12, 44, 24)
    ctx.fillStyle = accent
    ctx.font = '600 17px monospace'
    ctx.textAlign = 'right'
    ctx.fillText(value.toFixed(0), width - 6, cy + 6)

    ctx.fillStyle = muted
    ctx.font = '10px sans-serif'
    ctx.textAlign = 'center'
    ctx.fillText(label, width / 2, 12)
  }, [value, min, max, perPx, width, height, label, warnZone, theme])

  return <canvas ref={ref} style={{ width, height }} />
}