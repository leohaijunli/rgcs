// Plan summary strip (U17): a persistent, always-visible readout of the whole
// plan — survey area, pattern, item count, altitude datum and FC sync state —
// so the operator does not have to open every step to know what is set. It is
// read-only; every field is edited in its step card below.

import { useTranslation } from 'react-i18next'
import type { SyncPhase } from '../../../mission/sync'
import type { PlanBlocker } from '../../../mission/readiness'
import type { AltitudeMode } from '../../../stores/mission'
import type { PatternKind } from './PatternStep'
import SyncStatusLine from './SyncStatusLine'

export interface PlanSummaryProps {
  kind: PatternKind
  size: { width: number; height: number }
  heading: number
  /** Sweep line spacing (m) — the survey-density parameter. */
  lineSpacingM: number
  itemCount: number
  altitudeMode: AltitudeMode
  /** Lifecycle phase + hashes (U17d); rendered by the shared status line. */
  phase: SyncPhase
  planHash: string | null
  fcHash: string | null
  planHashFull: string | null
  fcHashFull: string | null
  /** Conditions that must be fixed (U17b); clicking one opens its step/field. */
  blockers: PlanBlocker[]
  onFix: (blocker: PlanBlocker) => void
}

export default function PlanSummary({
  kind,
  size,
  heading,
  lineSpacingM,
  itemCount,
  altitudeMode,
  phase,
  planHash,
  fcHash,
  planHashFull,
  fcHashFull,
  blockers,
  onFix,
}: PlanSummaryProps) {
  const { t } = useTranslation()
  const hasItems = itemCount > 0
  return (
    <div className="panel rounded px-2.5 py-2 text-[11px] leading-relaxed">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
        <span className="text-muted">{t('plan.summary.area')}</span>
        <span className="mono text-ink">
          {t('plan.summary.size', { w: size.width, h: size.height })}
        </span>
        <span className="mono text-muted">{t('plan.summary.az', { deg: heading })}</span>
        <span className="text-line">|</span>
        <span className="text-muted">{t(`plan.pattern.kind.${kind}`)}</span>
        {kind === 'sweep' && (
          <span className="mono text-muted">{t('plan.summary.spacing', { m: lineSpacingM })}</span>
        )}
      </div>
      <div className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5">
        <span className="text-muted">{t('plan.summary.items', { n: itemCount })}</span>
        <span className="text-muted">· {t(`plan.mode.${altitudeMode}`)}</span>
        <span className="flex-1" />
        {hasItems ? (
          <SyncStatusLine
            phase={phase}
            planHash={planHash}
            fcHash={fcHash}
            planHashFull={planHashFull}
            fcHashFull={fcHashFull}
          />
        ) : (
          <span className="text-muted">{t('plan.summary.noPlan')}</span>
        )}
      </div>
      {blockers.length > 0 && (
        <div className="mt-1.5 flex flex-col gap-1">
          {blockers.map((b) => (
            <button
              key={b.code}
              onClick={() => onFix(b)}
              title={b.detail ? `${t(`plan.blocker.${b.code}`)}: ${b.detail}` : t(`plan.blocker.${b.code}`)}
              className="flex items-start gap-1.5 rounded border border-error/50 bg-error/10 px-1.5 py-1 text-left text-[11px] text-error transition-colors hover:bg-error/20"
            >
              <span className="shrink-0">⚠</span>
              <span className="min-w-0 flex-1 leading-snug">{t(`plan.blocker.${b.code}`)}</span>
              <span className="shrink-0 text-muted">{t('plan.blocker.fix')}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
