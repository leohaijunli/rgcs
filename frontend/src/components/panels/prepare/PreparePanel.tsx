// Stage 1 — Prepare (U1): everything that must exist before planning — a
// live link, the mag calibration file, and the survey area's DEM (VIC data
// ⏸ M7). Reuses the pieces that already exist: the link store for status,
// the settings dialog for connecting, and the survey-prep card for the DEM.

import { useTranslation } from 'react-i18next'
import { Settings } from 'lucide-react'
import { useLinkStore } from '../../../stores/link'
import { useMissionStore } from '../../../stores/mission'
import { useUiStore } from '../../../stores/ui'
import { useTerrainStore } from '../../../stores/terrain'
import SurveyPrepCard from '../plan/SurveyPrepCard'
import { StatusDot } from '../../../design-system/components'

export default function PreparePanel() {
  const { t } = useTranslation()
  const link = useLinkStore((s) => s.link)
  const center = useMissionStore((s) => s.patternCenter)
  const mapCenter = useUiStore((s) => s.mapCenter)
  const size = useUiStore((s) => s.surveySize)
  const effectiveCenter = center ?? (mapCenter ? { latitude_deg: mapCenter.lat, longitude_deg: mapCenter.lon } : null)
  const tile = useTerrainStore((s) => s.tile)

  const rows: Array<{ label: string; ok: 'ok' | 'warn' | 'err' | 'off'; hint: string }> = [
    {
      label: t('prepare.link'),
      ok: link?.fc_alive ? 'ok' : 'err',
      hint: link?.fc_alive ? t('prepare.linkUp') : t('prepare.linkDown'),
    },
    {
      label: t('prepare.dem'),
      ok: tile ? 'ok' : 'warn',
      hint: tile ? tile.path.split(/[\\/]/).pop() ?? '' : t('prepare.demHint'),
    },
    {
      label: t('prepare.calibration'),
      ok: 'off',
      hint: t('prepare.calibrationHint'),
    },
  ]

  return (
    <div className="space-y-3" data-testid="prepare-panel">
      <div className="space-y-1.5">
        {rows.map((row) => (
          <div
            key={row.label}
            className="flex items-center justify-between rounded border border-line bg-canvas px-2 py-1.5 text-xs"
          >
            <span className="flex items-center gap-2 text-ink">
              <StatusDot tone={row.ok === 'off' ? 'muted' : row.ok === 'ok' ? 'ok' : row.ok === 'warn' ? 'warn' : 'error'} />
              {row.label}
            </span>
            <span className="truncate text-muted" title={row.hint}>
              {row.hint}
            </span>
          </div>
        ))}
      </div>
      <p className="flex items-center gap-1.5 text-[11px] text-muted">
        <Settings size={12} />
        {t('prepare.connectHint')}
      </p>
      {effectiveCenter && (
        <SurveyPrepCard center={effectiveCenter} size={size} />
      )}
    </div>
  )
}
