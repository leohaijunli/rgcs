import { useTranslation } from 'react-i18next'
import { useTelemetryStore } from '../stores/telemetry'
import { useDataAge } from '../hooks/useDataAge'
import AttitudeIndicator from './instruments/AttitudeIndicator'

const STALE_MS = 3000
const DIM_MS = 1000

/** Compact corner HUD: small attitude + key readouts. Not over the map center. */
export default function Hud() {
  const { t } = useTranslation()
  const snapshot = useTelemetryStore((s) => s.snapshot)
  const isMock = useTelemetryStore((s) => s.isMock)
  const age = useDataAge()
  const att = snapshot?.attitude ?? null
  const pos = snapshot?.global_position ?? null
  const battery = snapshot?.battery ?? null

  if (!snapshot) return null
  const speedMs = pos ? Math.hypot(pos.velocity.x_m_s, pos.velocity.y_m_s) : 0
  const stale = age > STALE_MS
  const dim = stale || age > DIM_MS

  return (
    <div
      className={`pointer-events-none absolute bottom-3 left-3 z-10 flex items-end gap-3 rounded border border-line bg-panel/85 p-2 shadow-xl transition-opacity ${dim ? 'opacity-45' : ''}`}
    >
      <AttitudeIndicator rollDeg={att?.roll_deg ?? 0} pitchDeg={att?.pitch_deg ?? 0} size={92} />
      <div className="grid grid-cols-2 gap-x-4 gap-y-1 pb-1">
        {stale && (
          <div className="col-span-2 rounded bg-error px-2 py-0.5 text-center text-[10px] font-medium text-white">
            STALE
          </div>
        )}
        {isMock && (
          <div className="col-span-2 rounded bg-canvas px-2 py-0.5 text-center text-[10px] text-warn">
            MOCK
          </div>
        )}
        <Readout label={t('hud.speed')} value={speedMs.toFixed(1)} unit="m/s" />
        <Readout label={t('hud.heading')} value={pos?.heading_deg.toFixed(0) ?? '—'} unit="°" />
        <Readout label="REL" value={pos?.relative_alt_m.toFixed(1) ?? '—'} unit="m" />
        <Readout label={t('hud.altAmsl')} value={pos?.altitude.meters.toFixed(1) ?? '—'} unit="m" />
        <Readout
          label={t('hud.battery')}
          value={battery?.remaining_percent != null ? String(battery.remaining_percent) : '—'}
          unit="%"
        />
        <Readout label="SV" value={snapshot.gps ? String(snapshot.gps.satellites_visible) : '—'} />
      </div>
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