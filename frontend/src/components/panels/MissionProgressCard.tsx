import { useTranslation } from 'react-i18next'
import { formatDistance, formatDuration } from '../../mission/progress'
import { useMissionProgress } from '../../hooks/useMissionProgress'

/**
 * Flight readout for the uploaded plan (issues.md #37): which waypoint the FC
 * is flying toward, how far it is, and how much of the mission is left.
 *
 * The planning panel lists waypoints to edit them; while flying the operator
 * needs the same plan expressed as progress, so this card is the flight-view
 * counterpart of that list.
 */
export default function MissionProgressCard() {
  const { t } = useTranslation()
  const { items, progress } = useMissionProgress()

  if (items.length === 0) {
    return <div className="panel rounded p-3 text-sm text-muted">{t('plan.empty')}</div>
  }

  const active = progress.targetIndex !== null ? progress.targetIndex + 1 : null
  const toNext =
    progress.toTargetM === null
      ? null
      : `${formatDistance(progress.toTargetM)}${progress.toTargetBearingDeg === null ? '' : ` · ${progress.toTargetBearingDeg.toFixed(0)}°`}`

  const rows: Array<[string, string]> = [
    [t('plan.progress.current'), active === null ? '—' : `${active} / ${progress.total}`],
    [t('plan.progress.toNext'), toNext ?? '—'],
    [
      t('plan.progress.targetAlt'),
      progress.targetAltitudeM === null ? '—' : `${progress.targetAltitudeM.toFixed(1)} m`,
    ],
    [
      t('plan.progress.remainingPath'),
      formatDistance(progress.remainingPathM) ?? '—',
    ],
    [t('plan.progress.eta'), formatDuration(progress.etaS) ?? '—'],
  ]

  const pct = Math.round(progress.fraction * 100)
  const hint =
    progress.targetIndex === null
      ? t('plan.progress.noCurrent')
      : progress.toTargetM === null
        ? t('plan.progress.noFix')
        : null

  return (
    <div className="panel rounded p-3">
      <div className="mb-2 text-xs uppercase tracking-wide text-muted">
        {t('plan.progress.title')}
      </div>
      <div className="space-y-1.5">
        {rows.map(([k, v]) => (
          <div key={k} className="flex items-baseline justify-between gap-3 text-sm">
            <span className="text-muted">{k}</span>
            <span className="mono">{v}</span>
          </div>
        ))}
      </div>
      <div
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        aria-label={t('plan.progress.title')}
        className="mt-2 h-1.5 w-full overflow-hidden rounded bg-canvas"
      >
        <div className="h-full rounded bg-accent" style={{ width: `${pct}%` }} />
      </div>
      <div className="mt-1 flex items-center justify-between text-[11px] text-muted">
        <span>{t('plan.progress.covered', { pct })}</span>
        <span className="mono">
          {t('plan.progress.counts', { flown: progress.flown, total: progress.total })}
        </span>
      </div>
      {hint && <div className="mt-1 text-[11px] text-muted">{hint}</div>}
    </div>
  )
}
