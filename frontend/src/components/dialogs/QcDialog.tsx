import { useEffect, useRef } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'
import { useTelemetryStore } from '../../stores/telemetry'
import { useUiStore } from '../../stores/ui'
import { cssVar } from '../../design-system/theme'

const WINDOW = 120 // samples
const STEP_MS = 250

/** Realtime QC curves in an independent popup (does not cover the map). */
export default function QcDialog() {
  const { t } = useTranslation()
  const open = useUiStore((s) => s.qcOpen)
  const setOpen = useUiStore((s) => s.setQcOpen)
  const hostRef = useRef<HTMLDivElement>(null)
  const plotRef = useRef<uPlot | null>(null)
  const dataRef = useRef<[number[], number[]]>([[0], [0]])

  useEffect(() => {
    const host = hostRef.current
    if (!host) return
    const opts: uPlot.Options = {
      width: 560,
      height: 240,
      legend: { show: true },
      series: [
        {},
        { label: 'SPD m/s', stroke: cssVar('--mg-accent'), width: 2 },
      ],
      axes: [
        { stroke: cssVar('--mg-muted'), grid: { stroke: cssVar('--mg-border') } },
        { stroke: cssVar('--mg-muted'), grid: { stroke: cssVar('--mg-border') } },
      ],
    }
    const plot = new uPlot(opts, dataRef.current, host)
    plotRef.current = plot
    return () => {
      plot.destroy()
      plotRef.current = null
    }
  }, [])

  // Feed from the telemetry store at a fixed cadence.
  useEffect(() => {
    if (!open) return
    const id = window.setInterval(() => {
      const snapshot = useTelemetryStore.getState().snapshot
      const pos = snapshot?.global_position
      const speed = pos ? Math.hypot(pos.velocity.x_m_s, pos.velocity.y_m_s) : 0
      const d = dataRef.current
      const now = d[0].length ? d[0][d[0].length - 1] + STEP_MS : 0
      d[0].push(now)
      d[1].push(speed)
      if (d[0].length > WINDOW) {
        d[0].shift()
        d[1].shift()
      }
      plotRef.current?.setData(d)
    }, STEP_MS)
    return () => window.clearInterval(id)
  }, [open])

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 w-[min(94vw,640px)] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-line bg-panel p-4 shadow-2xl">
          <div className="mb-2 flex items-center justify-between">
            <Dialog.Title className="text-sm font-medium text-ink">{t('dock.qc')}</Dialog.Title>
            <Dialog.Close asChild>
              <button
                aria-label="close"
                className="touch-target flex h-8 w-8 items-center justify-center rounded text-muted hover:text-ink"
              >
                <X size={16} />
              </button>
            </Dialog.Close>
          </div>
          <div ref={hostRef} />
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}