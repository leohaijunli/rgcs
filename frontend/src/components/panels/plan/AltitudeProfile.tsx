// Altitude profile (plan review, P2): the planned height line against the
// DEM ground under it, sampled along the flown path — the design-stage
// check for undulation and clearance without eyeballing the 3D globe.
// Collapsed by default; uPlot (already a dependency) draws it.

import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'
import { useMissionStore } from '../../../stores/mission'
import { useTerrainStore } from '../../../stores/terrain'
import { profileSeries } from '../../../mission/profile'
import { cssVar } from '../../../design-system/theme'

const HEIGHT_PX = 150

export default function AltitudeProfile() {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const hostRef = useRef<HTMLDivElement | null>(null)
  const plotRef = useRef<uPlot | null>(null)
  const waypoints = useMissionStore((s) => s.waypoints)
  const groundBySeq = useTerrainStore((s) => s.groundBySeq)

  useEffect(() => {
    if (!open) return
    const data = profileSeries(
      useMissionStore.getState().waypoints,
      useTerrainStore.getState().groundBySeq,
    )
    const host = hostRef.current
    if (!host) return
    if (!data) {
      plotRef.current?.destroy()
      plotRef.current = null
      host.textContent = t('plan.profile.noData')
      return
    }
    host.textContent = ''
    const w = host.clientWidth || 400
    const opts: uPlot.Options = {
      width: w,
      height: HEIGHT_PX,
      legend: { show: true, live: false },
      scales: { x: { time: false } },
      axes: [
        { stroke: cssVar('--mg-muted'), label: 'km', grid: { stroke: cssVar('--mg-border') } },
        { stroke: cssVar('--mg-muted'), label: 'm', grid: { stroke: cssVar('--mg-border') } },
      ],
      series: [
        {},
        {
          label: t('plan.profile.planned'),
          stroke: cssVar('--mg-accent'),
          width: 2,
        },
        {
          label: t('plan.profile.ground'),
          stroke: cssVar('--mg-ok'),
          width: 1.5,
          spanGaps: true,
        },
        {
          label: t('plan.profile.agl'),
          stroke: cssVar('--mg-muted'),
          width: 1,
          dash: [4, 3],
          scale: 'agl' as never,
        },
      ],
    }
    const series2 = data.amsl
    const series3 = data.ground
    const series4 = data.agl
    const plot = new uPlot(
      opts,
      [data.dist, series2, series3, series4] as unknown as uPlot.AlignedData,
      host,
    )
    plotRef.current = plot
    return () => {
      plot.destroy()
      plotRef.current = null
    }
  }, [open, waypoints, groundBySeq, t])

  return (
    <div className="rounded border border-line bg-panel p-2">
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center justify-between text-left text-xs text-muted hover:text-ink"
      >
        <span>{t('plan.profile.title')}</span>
        <span>{open ? '▾' : '▸'}</span>
      </button>
      {open && <div ref={hostRef} className="mt-1 min-h-[150px]" />}
    </div>
  )
}
