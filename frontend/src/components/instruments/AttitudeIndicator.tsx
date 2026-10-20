import { useEffect, useRef } from 'react'
import { cssVar } from '../../design-system/theme'
import { useUiStore } from '../../stores/ui'

interface Props {
  rollDeg: number
  pitchDeg: number
  size?: number
}

/** Artificial horizon: roll rotates the scene, pitch translates it. */
export default function AttitudeIndicator({ rollDeg, pitchDeg, size = 190 }: Props) {
  const ref = useRef<HTMLCanvasElement>(null)
  const theme = useUiStore((s) => s.theme)

  useEffect(() => {
    const canvas = ref.current
    if (!canvas) return
    const dpr = window.devicePixelRatio || 1
    canvas.width = size * dpr
    canvas.height = size * dpr
    const ctx = canvas.getContext('2d')
    if (!ctx) return
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)

    const c = size / 2
    const r = size * 0.47
    const rad = Math.PI / 180
    const sky = cssVar('--mg-sky')
    const ground = cssVar('--mg-ground')
    const ink = cssVar('--mg-ink')
    const muted = cssVar('--mg-muted')

    ctx.clearRect(0, 0, size, size)
    ctx.save()
    ctx.beginPath()
    ctx.arc(c, c, r, 0, Math.PI * 2)
    ctx.clip()

    ctx.translate(c, c)
    ctx.rotate(-rollDeg * rad)
    ctx.translate(0, pitchDeg * (size / 55))

    // Sky and ground.
    ctx.fillStyle = sky
    ctx.fillRect(-size, -size, size * 2, size * 2)
    ctx.fillStyle = ground
    ctx.fillRect(-size, 0, size * 2, size * 2)

    // Horizon line.
    ctx.strokeStyle = ink
    ctx.lineWidth = 2
    ctx.beginPath()
    ctx.moveTo(-size, 0)
    ctx.lineTo(size, 0)
    ctx.stroke()

    // Pitch ladder.
    ctx.strokeStyle = ink
    ctx.lineWidth = 1
    ctx.fillStyle = ink
    for (let p = -30; p <= 30; p += 10) {
      if (p === 0) continue
      const y = -p * (size / 55)
      const w = p % 30 === 0 ? size * 0.32 : size * 0.2
      ctx.beginPath()
      ctx.moveTo(-w, y)
      ctx.lineTo(w, y)
      ctx.stroke()
      ctx.font = '11px sans-serif'
      ctx.textAlign = 'center'
      ctx.fillText(String(p), w + 16, y + 4)
      ctx.fillText(String(-p), -w - 16, y + 4)
    }

    ctx.restore()

    // Fixed aircraft symbol.
    ctx.strokeStyle = ink
    ctx.lineWidth = 2
    ctx.beginPath()
    ctx.moveTo(c - 16, c)
    ctx.lineTo(c + 16, c)
    ctx.moveTo(c, c - 6)
    ctx.lineTo(c, c + 10)
    ctx.stroke()

    // Rim + roll reference marks.
    ctx.strokeStyle = muted
    ctx.lineWidth = 2
    ctx.beginPath()
    ctx.arc(c, c, r, 0, Math.PI * 2)
    ctx.stroke()

    ctx.strokeStyle = muted
    for (const a of [-30, -20, -10, 10, 20, 30]) {
      const t = a * rad
      ctx.beginPath()
      ctx.moveTo(c + Math.sin(t) * r * 0.86, c - Math.cos(t) * r * 0.86)
      ctx.lineTo(c + Math.sin(t) * r, c - Math.cos(t) * r)
      ctx.stroke()
    }
    ctx.beginPath()
    ctx.arc(c, c, r * 0.92, 0, Math.PI * 2)
    ctx.stroke()
  }, [rollDeg, pitchDeg, size, theme])

  return <canvas ref={ref} style={{ width: size, height: size }} />
}