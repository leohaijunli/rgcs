// Drape preview (DR4): the designed profile per line against the DTM, the
// DSM and the clearance floor, with the acceptance band and the out-of-band
// spans shaded. Data comes from the solver preview (`mission.drapePreview`),
// not from the interim waypoint drape. Below the chart: the AGL stats table
// and the crossing consistency — the numbers the operator checks before
// trusting the plan.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import uPlot from 'uplot'
import 'uplot/dist/uPlot.min.css'
import { useMissionStore } from '../../../stores/mission'
import { cssVar } from '../../../design-system/theme'

const HEIGHT_PX = 140

/** Infeasibility reason → the badge the table shows. */
function reasonLabel(
  reason: 'clearance' | 'climb_rate' | 'descent_rate' | 'no_data',
  t: (k: string) => string,
): string {
  return t(`plan.pattern.drapePreview.reason.${reason}`)
}

export default function DrapePreview() {
  const { t } = useTranslation()
  const preview = useMissionStore((s) => s.drapePreview)
  const [lineIdx, setLineIdx] = useState(0)
  const hostRef = useRef<HTMLDivElement | null>(null)
  const plotRef = useRef<uPlot | null>(null)

  const lines = preview?.result.lines ?? []
  const line = lines[Math.min(lineIdx, Math.max(lines.length - 1, 0))]

  useEffect(() => {
    if (lineIdx >= lines.length) setLineIdx(0)
  }, [lines.length, lineIdx])

  useEffect(() => {
    const host = hostRef.current
    if (!host) return
    if (!line || line.profile.length < 2) {
      plotRef.current?.destroy()
      plotRef.current = null
      host.textContent = t('plan.pattern.drapePreview.noData')
      return
    }
    const dist = line.profile.map((s) => s.s)
    const zPlan = line.profile.map((s) => s.z_plan)
    const dtm = line.profile.map((s) => s.dtm)
    const dsm: Array<number | null> = line.profile.map((s) => s.dsm)
    const floor = dsm.map((d) => (d == null ? null : d + 5))
    const bandHi = line.profile.map((s) => s.dtm + 43)
    const bandLo = line.profile.map((s) => s.dtm + 37)

    const w = host.clientWidth || 260
    const opts: uPlot.Options = {
      width: w,
      height: HEIGHT_PX,
      legend: { show: false },
      scales: { x: { time: false } },
      axes: [
        { stroke: cssVar('--mg-muted'), label: 'm', grid: { stroke: cssVar('--mg-border') } },
        { stroke: cssVar('--mg-muted'), grid: { stroke: cssVar('--mg-border') } },
      ],
      series: [
        {},
        { label: t('plan.profile.planned'), stroke: cssVar('--mg-accent'), width: 2 },
        { label: t('plan.profile.ground'), stroke: cssVar('--mg-ok'), width: 1.5, spanGaps: true },
        { label: t('plan.pattern.drapePreview.dsm'), stroke: cssVar('--mg-warn'), width: 1, spanGaps: true },
        { label: t('plan.pattern.drapePreview.floor'), stroke: cssVar('--mg-error'), width: 1, dash: [3, 3], spanGaps: true },
        { label: t('plan.pattern.drapePreview.band'), stroke: cssVar('--mg-muted'), width: 0, fill: 'transparent', spanGaps: true },
        { label: '', stroke: cssVar('--mg-muted'), width: 0, spanGaps: true },
      ],
    }
    host.textContent = ''
    const plot = new uPlot(
      opts,
      [dist, zPlan, dtm, dsm, floor, bandHi, bandLo] as unknown as uPlot.AlignedData,
      host,
    )
    plotRef.current = plot
    return () => {
      plot.destroy()
      plotRef.current = null
    }
  }, [line, t])

  const stats = preview?.result.stats
  const crossings = preview?.result.crossings ?? []

  const lineOptions = useMemo(
    () => lines.map((l, i) => ({ id: l.line_id, index: i })),
    [lines],
  )

  if (!preview) return null

  return (
    <div className="space-y-1.5 rounded border border-line bg-canvas p-2" data-testid="drape-preview">
      <div className="flex items-center justify-between text-xs">
        <span className="text-muted">{t('plan.pattern.drapePreview.title')}</span>
        {lines.length > 1 && (
          <select
            value={lineIdx}
            onChange={(e) => setLineIdx(Number(e.target.value))}
            className="rounded border border-line bg-panel px-1 py-0.5 text-[11px] text-ink"
            aria-label={t('plan.pattern.drapePreview.line')}
          >
            {lineOptions.map((o) => (
              <option key={o.id} value={o.index}>
                {o.id}
              </option>
            ))}
          </select>
        )}
      </div>
      <div ref={hostRef} className="min-h-[140px]" />

      {/* Out-of-band spans: highlighted, each with the deficit and the binder.
       * U5: the operator can accept them here or from the preflight panel —
       * one store flag, invalidated by any new solve. */}
      {(preview.result.infeasible ?? []).length > 0 && (
        <div className="space-y-1" data-testid="infeasible-spans">
          {preview.result.infeasible.map((sp, i) => (
            <div key={i} className="rounded border border-error bg-canvas px-2 py-1 text-[11px] text-error">
              {t('plan.pattern.drapePreview.span', {
                line: sp.line_id,
                from: sp.s_start.toFixed(0),
                to: sp.s_end.toFixed(0),
                deficit: sp.max_deficit_m.toFixed(1),
                reason: reasonLabel(sp.reason, t),
              })}
            </div>
          ))}
          <InfeasibleAcceptRow />
        </div>
      )}

      {stats && (
        <table className="w-full text-[11px]" data-testid="drape-stats">
          <tbody>
            <tr>
              <td className="text-muted">{t('plan.pattern.drapePreview.aglRange')}</td>
              <td className="mono text-right">
                {stats.agl_min_m.toFixed(1)}–{stats.agl_max_m.toFixed(1)} m
              </td>
            </tr>
            <tr>
              <td className="text-muted">{t('plan.pattern.drapePreview.aglMean')}</td>
              <td className="mono text-right">
                {stats.agl_mean_m.toFixed(1)} ± {stats.agl_std_m.toFixed(1)} m
              </td>
            </tr>
            {stats.min_clearance_m != null && (
              <tr>
                <td className="text-muted">{t('plan.pattern.drapePreview.minClearance')}</td>
                <td className="mono text-right">{stats.min_clearance_m.toFixed(1)} m</td>
              </tr>
            )}
            <tr>
              <td className="text-muted">{t('plan.pattern.drapePreview.maxClimb')}</td>
              <td className="mono text-right">{stats.max_climb_needed_mps.toFixed(1)} m/s</td>
            </tr>
            <tr>
              <td className="text-muted">{t('plan.pattern.drapePreview.crossings')}</td>
              <td className="mono text-right">
                {crossings.length === 0
                  ? '—'
                  : crossings
                      .map((c) => Math.abs(c.agl_diff_m).toFixed(2))
                      .slice(0, 4)
                      .join(', ')}
              </td>
            </tr>
          </tbody>
        </table>
      )}
      {!preview.has_dtm && (
        <div className="text-[11px] text-warn">{t('plan.pattern.drapePreview.noDtm')}</div>
      )}
    </div>
  )
}

/** The infeasible-acceptance row (U5): accepting records the `spec_hash`, and
 * the preflight checklist then passes `infeasible_confirmed` for this solve. */
function InfeasibleAcceptRow() {
  const { t } = useTranslation()
  const preview = useMissionStore((s) => s.drapePreview)
  const acceptedFor = useMissionStore((s) => s.infeasibleAcceptedFor)
  const accept = useMissionStore((s) => s.acceptInfeasible)
  if (!preview) return null
  const accepted = acceptedFor === preview.result.spec_hash
  return (
    <button
      data-testid="drape-accept-infeasible"
      onClick={() => accept(preview.result.spec_hash)}
      disabled={accepted}
      className="touch-target w-full rounded border border-line bg-canvas px-2 py-1 text-[11px] text-accent hover:bg-panel disabled:opacity-50"
    >
      {accepted
        ? t('preflight.infeasibleAccepted')
        : t('preflight.acceptInfeasible', { count: preview.result.infeasible.length })}
    </button>
  )
}
