// FC-sync status line (U17d): the lifecycle phrase plus the compact plan / FC
// hash readout, so "does the FC hold this plan?" and "which plan?" are both
// answerable at a glance without opening the Sync step. Shared by the plan
// summary strip (PlanSummary) and the Sync step so the wording and tone cannot
// drift between the two.

import { useTranslation } from 'react-i18next'
import type { SyncPhase } from '../../../mission/sync'

/** Tone of each phase (text comes from `plan.sync.*`). */
const PHASE_TONE: Record<SyncPhase, string> = {
  empty: 'text-muted',
  unsynced: 'text-muted',
  dirty: 'text-warn',
  uploading: 'text-accent',
  verifying: 'text-accent',
  downloading: 'text-accent',
  clearing: 'text-warn',
  mismatch: 'text-error',
  synced: 'text-ok',
}

export interface SyncStatusLineProps {
  phase: SyncPhase
  /** Compact hash of the local plan; null when there is nothing to sync. */
  planHash: string | null
  /** Compact hash of the last-synced baseline; null when never synced. */
  fcHash: string | null
  /** Full canonical strings for the tooltips (hover to compare verbatim). */
  planHashFull?: string | null
  fcHashFull?: string | null
}

export default function SyncStatusLine({
  phase,
  planHash,
  fcHash,
  planHashFull,
  fcHashFull,
}: SyncStatusLineProps) {
  const { t } = useTranslation()
  return (
    <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-0.5">
      <span className={PHASE_TONE[phase]}>{t(`plan.sync.${phase}`)}</span>
      {planHash && (
        <span className="mono text-muted" title={planHashFull ?? undefined}>
          {t('plan.sync.hashPlan')} {planHash}
        </span>
      )}
      {fcHash && (
        <span className="mono text-muted" title={fcHashFull ?? undefined}>
          {t('plan.sync.hashFc')} {fcHash}
        </span>
      )}
    </span>
  )
}
