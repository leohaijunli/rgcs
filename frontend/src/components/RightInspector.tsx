import { useTranslation } from 'react-i18next'
import PlanningPanel from './panels/PlanningPanel'
import MissionProgressCard from './panels/MissionProgressCard'
import { useLinkStore } from '../stores/link'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore, VIEW_CONFIG } from '../stores/ui'
import { modeLabel } from '../util/modeLabel'

export default function RightInspector() {
  const { t } = useTranslation()
  const view = useUiStore((s) => s.view)
  const rightOpen = useUiStore((s) => s.rightOpen)
  const toggleRight = useUiStore((s) => s.toggleRight)

  if (!rightOpen) return null

  const tab = VIEW_CONFIG[view].rightTab

  return (
    <aside className="flex w-[264px] shrink-0 flex-col border-l border-line bg-panel">
      <div className="flex items-center justify-between border-b border-line px-3 py-2">
        <h2 className="text-sm font-medium">{t(tab)}</h2>
        <button
          onClick={toggleRight}
          className="touch-target rounded px-2 text-sm text-muted hover:text-ink"
        >
          ×
        </button>
      </div>
      <div className="flex-1 overflow-y-auto p-3">
        {view === 'planning' ? (
          <PlanningPanel />
        ) : view === 'flight' ? (
          <FlightInspector />
        ) : (
          <GenericInspector view={view} />
        )}
      </div>
    </aside>
  )
}

function FlightInspector() {
  const { t } = useTranslation()
  const link = useLinkStore((s) => s.link)
  const heartbeat = useTelemetryStore((s) => s.snapshot?.heartbeat ?? null)
  const pos = useTelemetryStore((s) => s.snapshot?.global_position ?? null)
  const battery = useTelemetryStore((s) => s.snapshot?.battery ?? null)
  const gps = useTelemetryStore((s) => s.snapshot?.gps ?? null)

  const speedMs = pos ? Math.hypot(pos.velocity.x_m_s, pos.velocity.y_m_s) : 0

  return (
    <div className="space-y-2">
      <MissionProgressCard />
      <Card
        title={t('panels.telemetry')}
        rows={[
          [t('link.mode'), modeLabel(t, heartbeat) ?? '—'],
          [t('link.armed'), heartbeat?.base_mode.safety_armed ? t('link.armed') : t('link.disarmed')],
          [t('hud.speed'), speedMs ? `${speedMs.toFixed(1)} m/s` : '—'],
          [t('hud.alt'), pos ? `${pos.relative_alt_m.toFixed(1)} m` : '—'],
          [t('hud.altAmsl'), pos ? `${pos.altitude.meters.toFixed(1)} m` : '—'],
          [t('hud.heading'), pos ? `${pos.heading_deg.toFixed(0)}°` : '—'],
          [t('hud.battery'), battery?.remaining_percent != null ? `${battery.remaining_percent}%` : '—'],
          [t('hud.gps'), gps ? t(`hud.fix.${fixKey(gps.fix_type)}`) : '—'],
          [t('link.fc'), link?.fc_alive ? t('link.connected') : t('link.lost')],
        ]}
      />
    </div>
  )
}

function GenericInspector({ view }: { view: 'planning' | 'data' }) {
  const { t } = useTranslation()
  return (
    <div className="panel rounded p-3 text-sm text-muted">
      {view === 'planning'
        ? t('views.planningPlaceholder')
        : t('views.dataPlaceholder')}
    </div>
  )
}

function Card({ title, rows }: { title: string; rows: Array<[string, string]> }) {
  return (
    <div className="panel rounded p-3">
      <div className="mb-2 text-xs uppercase tracking-wide text-muted">{title}</div>
      <div className="space-y-1.5">
        {rows.map(([k, v]) => (
          <div key={k} className="flex items-baseline justify-between gap-3 text-sm">
            <span className="text-muted">{k}</span>
            <span className="mono">{v}</span>
          </div>
        ))}
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
