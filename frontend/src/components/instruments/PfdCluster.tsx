import { useTranslation } from 'react-i18next'
import { useTelemetryStore } from '../../stores/telemetry'
import AttitudeIndicator from './AttitudeIndicator'
import VerticalTape from './VerticalTape'

/**
 * Metric instrument cluster for the dashboard popup.
 * Self-authored canvas instruments so both themes apply (no third-party
 * imperial dials). Vertical speed uses the NED down-speed: climb = -vz.
 */
export default function PfdCluster() {
  const { t } = useTranslation()
  const snapshot = useTelemetryStore((s) => s.snapshot)
  const att = snapshot?.attitude ?? null
  const pos = snapshot?.global_position ?? null
  const gps = snapshot?.gps ?? null
  const battery = snapshot?.battery ?? null

  const speedMs = pos ? Math.hypot(pos.velocity.x_m_s, pos.velocity.y_m_s) : 0
  const vsMs = pos ? -pos.velocity.z_m_s : 0
  const relAlt = pos?.relative_alt_m ?? 0
  const heading = pos?.heading_deg ?? 0

  return (
    <div className="flex flex-col items-center gap-4">
      <div className="flex items-end gap-3">
        <VerticalTape
          label={t('hud.speed')}
          value={speedMs}
          min={0}
          max={50}
          perPx={0.18}
          width={72}
          height={240}
        />
        <AttitudeIndicator rollDeg={att?.roll_deg ?? 0} pitchDeg={att?.pitch_deg ?? 0} size={230} />
        <VerticalTape
          label="REL"
          value={relAlt}
          min={-10}
          max={150}
          perPx={0.6}
          width={72}
          height={240}
          warnZone={[3, 120]}
        />
      </div>

      <div className="flex items-center justify-center gap-6">
        <Readout label={t('hud.heading')} value={heading.toFixed(0)} unit="°" />
        <Readout label="VSI" value={vsMs.toFixed(1)} unit="m/s" sign={vsMs > 0.05 ? '▲' : vsMs < -0.05 ? '▼' : ''} />
        <Readout label={t('hud.altAmsl')} value={(pos?.altitude.meters ?? 0).toFixed(1)} unit="m" />
        <Readout
          label={t('hud.battery')}
          value={battery?.remaining_percent != null ? String(battery.remaining_percent) : '—'}
          unit="%"
        />
        <Readout label="GPS" value={gps ? t(`hud.fix.${fixKey(gps.fix_type)}`) : '—'} />
        <Readout label="SV" value={gps ? String(gps.satellites_visible) : '—'} />
      </div>
    </div>
  )
}

function Readout({
  label,
  value,
  unit,
  sign,
}: {
  label: string
  value: string
  unit?: string
  sign?: string
}) {
  return (
    <div className="min-w-16 rounded border border-line bg-canvas px-3 py-1.5 text-center">
      <div className="text-[10px] uppercase tracking-wide text-muted">{label}</div>
      <div className="mono text-sm">
        {sign && <span className="mr-1 text-accent">{sign}</span>}
        {value}
        {unit && <span className="text-xs text-muted">{unit}</span>}
      </div>
    </div>
  )
}

function fixKey(fix: string): string {
  const map: Record<string, string> = {
    NO_GPS: 'noGps',
    NO_FIX: 'noFix',
    FIX2D: 'fix2d',
    FIX3D: 'fix3d',
    DGPS: 'dgps',
    RTK_FLOAT: 'rtkFloat',
    RTK_FIXED: 'rtkFixed',
    STATIC: 'static',
    PPP: 'ppp',
  }
  return map[fix] ?? 'noGps'
}