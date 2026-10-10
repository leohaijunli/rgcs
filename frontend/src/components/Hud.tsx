import { useTranslation } from 'react-i18next'
import { ChevronDown, ChevronUp } from 'lucide-react'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore } from '../stores/ui'
import { FIELD_STALE_MS, useDataAge, useFieldAge } from '../hooks/useDataAge'
import AttitudeIndicator from './instruments/AttitudeIndicator'

const STALE_MS = 3000
const DIM_MS = 1000

/** Flight-dynamics HUD, bottom-left above the Cesium credit (§4.5): attitude,
 * speed, heading and altitudes only — battery and satellite count are TopBar
 * health (design rule 2). Collapses to a small pill. */
export default function Hud() {
  const { t } = useTranslation()
  const snapshot = useTelemetryStore((s) => s.snapshot)
  const collapsed = useUiStore((s) => s.hudCollapsed)
  const toggleHud = useUiStore((s) => s.toggleHud)
  const age = useDataAge()
  const gpsAge = useFieldAge(snapshot?.field_ages.gps_at_ms)
  const att = snapshot?.attitude ?? null
  const pos = snapshot?.global_position ?? null

  if (!snapshot) return null
  const speedMs = pos ? Math.hypot(pos.velocity.x_m_s, pos.velocity.y_m_s) : 0
  const stale = age > STALE_MS
  const dim = stale || age > DIM_MS
  const gpsStale = snapshot != null && gpsAge > FIELD_STALE_MS

  if (collapsed) {
    return (
      <div className="absolute bottom-3 left-3 z-10">
        <button
          onClick={toggleHud}
          title={t('hud.expand')}
          aria-label={t('hud.expand')}
          className="flex items-center gap-1.5 rounded border border-line bg-panel/85 px-2 py-1 text-xs text-muted shadow-xl hover:text-ink"
        >
          <ChevronUp size={12} />
          {t('hud.speed')} {speedMs.toFixed(1)} m/s
        </button>
      </div>
    )
  }

  return (
    <div
      className={`absolute bottom-3 left-3 z-10 flex items-end gap-3 rounded border border-line bg-panel/85 p-2 shadow-xl transition-opacity ${dim ? 'opacity-45' : ''}`}
    >
      <AttitudeIndicator rollDeg={att?.roll_deg ?? 0} pitchDeg={att?.pitch_deg ?? 0} size={92} />
      <div className="grid grid-cols-2 gap-x-4 gap-y-1 pb-1">
        {stale && (
          <div className="col-span-2 rounded bg-error px-2 py-0.5 text-center text-[10px] font-medium text-white">
            STALE
          </div>
        )}
        {gpsStale && (
          <div className="col-span-2 rounded bg-warn px-2 py-0.5 text-center text-[10px] text-canvas">
            GPS STALE
          </div>
        )}
        <Readout label={t('hud.speed')} value={speedMs.toFixed(1)} unit="m/s" />
        <Readout label={t('hud.heading')} value={pos?.heading_deg.toFixed(0) ?? '—'} unit="°" />
        <Readout label="REL" value={pos?.relative_alt_m.toFixed(1) ?? '—'} unit="m" />
        <Readout label={t('hud.altAmsl')} value={pos?.altitude.meters.toFixed(1) ?? '—'} unit="m" />
        <Readout
          label={t('hud.roll')}
          value={att?.roll_deg != null ? att.roll_deg.toFixed(0) : '—'}
          unit="°"
        />
        <Readout
          label={t('hud.pitch')}
          value={att?.pitch_deg != null ? att.pitch_deg.toFixed(0) : '—'}
          unit="°"
        />
      </div>
      <button
        onClick={toggleHud}
        title={t('hud.collapse')}
        aria-label={t('hud.collapse')}
        className="absolute -top-2 right-0 -translate-y-full rounded border border-line bg-panel px-1 py-0.5 text-muted hover:text-ink"
      >
        <ChevronDown size={12} />
      </button>
    </div>
  )
}

function Readout({ label, value, unit }: { label: string; value: string; unit?: string }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <span className="text-[10px] uppercase text-muted">{label}</span>
      <span className="mono text-xs text-ink">
        {value}
        {unit && <span className="text-[10px] text-muted">{unit}</span>}
      </span>
    </div>
  )
}