// Stage 1 — Prepare (U1): everything that must exist before planning — a
// live link, the mag calibration file, and the survey area's DEM (VIC data
// ⏸ M7). Reuses the pieces that already exist: the link store for status,
// the settings dialog for connecting, and the survey-prep card for the DEM.
//
// L8 adds the startup self-check rows: the last endpoint, the persisted mag
// calibration (hash prefix), the 3D terrain service, serial access (devices
// + udev rules) and the system clock — the things a field launch needs to
// be right before any planning happens.

import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Settings } from 'lucide-react'
import { invoke } from '@tauri-apps/api/core'
import { useLinkStore } from '../../../stores/link'
import { useMissionStore } from '../../../stores/mission'
import { useUiStore } from '../../../stores/ui'
import { useTerrainStore } from '../../../stores/terrain'
import { usePrefsStore } from '../../../desktop/prefs'
import { isTauri } from '../../../inspector/mock'
import SurveyPrepCard from '../plan/SurveyPrepCard'
import { StatusDot } from '../../../design-system/components'

interface ClockRow {
  epoch_ms: number
  timezone: string | null
}

export default function PreparePanel() {
  const { t } = useTranslation()
  const link = useLinkStore((s) => s.link)
  const center = useMissionStore((s) => s.patternCenter)
  const mapCenter = useUiStore((s) => s.mapCenter)
  const size = useUiStore((s) => s.surveySize)
  const effectiveCenter = center ?? (mapCenter ? { latitude_deg: mapCenter.lat, longitude_deg: mapCenter.lon } : null)
  const tile = useTerrainStore((s) => s.tile)
  const terrain3d = useTerrainStore((s) => s.terrain3d)
  const endpoint = usePrefsStore((s) => s.endpoint)

  // L8 self-check facts: the calibration hash and the clock live in the
  // backend; the udev check is Linux-only (the row says n/a elsewhere).
  const [calHash, setCalHash] = useState<string | null>(null)
  const [clock, setClock] = useState<ClockRow | null>(null)
  const [udev, setUdev] = useState<boolean | null>(null)
  useEffect(() => {
    if (!isTauri()) return
    void invoke<{ loaded: boolean; hash8: string | null }>('mag_calibration_status')
      .then((s) => setCalHash(s.hash8))
      .catch(() => {})
    void invoke<ClockRow>('clock_info')
      .then(setClock)
      .catch(() => {})
    void invoke<boolean | null>('udev_rules_present')
      .then(setUdev)
      .catch(() => {})
  }, [])

  const rows: Array<{ label: string; ok: 'ok' | 'warn' | 'err' | 'off'; hint: string }> = [
    {
      label: t('prepare.link'),
      ok: link?.fc_alive ? 'ok' : 'err',
      hint: link?.fc_alive ? t('prepare.linkUp') : t('prepare.linkDown'),
    },
    {
      label: t('prepare.endpoint'),
      ok: 'off',
      hint: endpoint,
    },
    {
      label: t('prepare.calibration'),
      ok: calHash ? 'ok' : 'warn',
      hint: calHash ? t('prepare.calibrationLoaded', { hash: calHash }) : t('prepare.calibrationHint'),
    },
    {
      label: t('prepare.dem'),
      ok: tile ? 'ok' : 'warn',
      hint: tile ? tile.path.split(/[\\/]/).pop() ?? '' : t('prepare.demHint'),
    },
    {
      label: t('prepare.terrain3d'),
      ok: terrain3d ? 'ok' : 'off',
      hint: terrain3d ? t('prepare.terrain3dUp') : t('prepare.terrain3dDown'),
    },
    {
      label: t('prepare.serial'),
      ok: udev === null ? 'off' : udev ? 'ok' : 'warn',
      hint:
        udev === null
          ? t('prepare.serialUnknown')
          : udev
            ? t('prepare.serialRulesOk')
            : t('prepare.serialRulesMissing'),
    },
    {
      label: t('prepare.clock'),
      ok: 'off',
      hint: clock
        ? clock.timezone
          ? t('prepare.clockWithZone', { zone: clock.timezone })
          : t('prepare.clockNoZone')
        : t('prepare.clockUnknown'),
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
