// The Fly-stage Drape panel (DR6): the DR5 monitor's state as the operator
// reads it — current line and progress, the e_agl band bar, actual and
// look-ahead clearance, ground speed with the saturation flag, the radar-vs-
// DEM difference, and the cumulative in-band fraction. Stale/unknown shows
// STALE + why, never a value that could read as healthy.

import { useTranslation } from 'react-i18next'
import { useDrapeMonitorStore } from '../../../stores/drapeMonitor'
import { useMissionStore } from '../../../stores/mission'
import { StatusDot } from '../../../design-system/components'

/** One readout row: label left, mono value right, tone by condition. */
function Row({ label, value, tone = 'muted' }: { label: string; value: string; tone?: 'ok' | 'warn' | 'error' | 'muted' }) {
  const cls = tone === 'ok' ? 'text-ok' : tone === 'warn' ? 'text-warn' : tone === 'error' ? 'text-error' : 'text-muted'
  return (
    <div className="flex items-center justify-between text-xs">
      <span className={cls}>{label}</span>
      <span className={`mono ${cls}`}>{value}</span>
    </div>
  )
}

/** The ±band bar with the current e_agl needle. */
function BandBar({ eAgl, band }: { eAgl: number | null; band: number }) {
  const span = Math.max(band * 3, 1)
  const clamped = eAgl == null ? 0 : Math.max(-span, Math.min(span, eAgl))
  const pct = 50 + (clamped / span) * 50
  const out = eAgl != null && Math.abs(eAgl) > band
  return (
    <div className="relative h-3 rounded border border-line bg-canvas" data-testid="drape-agl-bar">
      {/* the in-band zone */}
      <div
        className="absolute inset-y-0 bg-ok/15"
        style={{ left: `${50 - (band / span) * 50}%`, width: `${(2 * band * 50) / span}%` }}
      />
      {/* the needle */}
      <div
        className={`absolute inset-y-0 w-0.5 ${out ? 'bg-error' : 'bg-ok'}`}
        style={{ left: `${pct}%` }}
        data-testid="drape-agl-needle"
      />
    </div>
  )
}

export default function DrapePanel() {
  const { t } = useTranslation()
  const mon = useDrapeMonitorStore((s) => s.state)
  const running = useDrapeMonitorStore((s) => s.running)
  const ack = useDrapeMonitorStore((s) => s.ack)
  const spec = useMissionStore((s) => s.drapeSpec)

  if (!running || !mon) {
    return (
      <div className="rounded border border-line bg-canvas p-2 text-[11px] text-muted" data-testid="drape-panel">
        {t('drapePanel.notRunning')}
      </div>
    )
  }

  const fresh = mon.fresh === 'fresh'
  const clearance = mon.clearance_m
  const lookahead = mon.lookahead_clearance_m
  const clearanceTone =
    clearance == null ? 'muted' : clearance < (spec.minClearanceM ?? 5) ? 'error' : 'ok'
  const lookaheadTone =
    lookahead == null ? 'muted' : lookahead < 0 ? 'error' : lookahead < 5 ? 'warn' : 'ok'
  const eAgl = mon.e_agl_m
  const staleText = mon.fresh === 'stale' ? t('drapePanel.stale') : mon.fresh === 'outside_dem' ? t('drapePanel.outsideDem') : null

  return (
    <div
      className={`space-y-1.5 rounded border p-2 ${mon.latched_critical ? 'border-error' : 'border-line'} bg-canvas ${fresh ? '' : 'opacity-60'}`}
      data-testid="drape-panel"
    >
      <div className="flex items-center justify-between text-xs">
        <span className="font-medium text-ink">{t('drapePanel.title')}</span>
        <span className="flex items-center gap-1.5 text-muted">
          <StatusDot tone={mon.latched_critical ? 'error' : fresh ? 'ok' : 'muted'} />
          {mon.line_id != null ? `${mon.line_id} · ${Math.round((mon.progress ?? 0) * 100)}%` : '—'}
        </span>
      </div>
      {staleText != null && <div className="text-[11px] text-warn">{staleText}</div>}

      <BandBar eAgl={eAgl} band={3} />
      <Row
        label={t('drapePanel.eAgl')}
        value={eAgl != null ? `${eAgl >= 0 ? '+' : ''}${eAgl.toFixed(1)} m` : '—'}
        tone={eAgl != null && Math.abs(eAgl) > 3 ? 'warn' : 'ok'}
      />
      <Row
        label={t('drapePanel.clearance')}
        value={clearance != null ? `${clearance.toFixed(1)} m` : '—'}
        tone={clearanceTone as 'ok' | 'warn' | 'error' | 'muted'}
      />
      <Row
        label={t('drapePanel.lookahead')}
        value={lookahead != null ? `${lookahead.toFixed(1)} m` : '—'}
        tone={lookaheadTone as 'ok' | 'warn' | 'error' | 'muted'}
      />
      <Row
        label={t('drapePanel.crossTrack')}
        value={mon.cross_track_m != null ? `${mon.cross_track_m.toFixed(1)} m` : '—'}
      />
      <Row
        label={t('drapePanel.radarDiff')}
        value={mon.e_dem_median_m != null ? `${mon.e_dem_median_m.toFixed(1)} ± ${mon.e_dem_mad_m?.toFixed(1) ?? '?'} m` : '—'}
      />
      <Row
        label={t('drapePanel.inBand')}
        value={`${Math.round(mon.in_band_ratio * 100)}%`}
        tone={mon.in_band_ratio > 0.8 ? 'ok' : 'warn'}
      />
      {mon.saturated && <div className="text-[11px] text-warn">{t('drapePanel.saturated')}</div>}
      {mon.latched_critical && (
        <button
          onClick={() => void ack()}
          className="w-full rounded border border-error px-2 py-1 text-[11px] text-error hover:bg-panel"
          data-testid="drape-critical-ack"
        >
          {t('drapePanel.acknowledge')}
        </button>
      )}
    </div>
  )
}
