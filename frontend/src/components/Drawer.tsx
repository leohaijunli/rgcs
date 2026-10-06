import { useTranslation } from 'react-i18next'
import { useLinkStore } from '../stores/link'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore, type DrawerId } from '../stores/ui'
import { fcStatusLabel } from '../util/linkLabel'

export default function Drawer() {
  const { t } = useTranslation()
  const drawer = useUiStore((s) => s.drawer)
  const closeDrawer = useUiStore((s) => s.closeDrawer)

  if (!drawer) return null

  return (
    <aside
      className="flex w-[280px] shrink-0 flex-col border-r border-line bg-panel"
      style={{ width: 280 }}
    >
      <div className="flex items-center justify-between border-b border-line px-3 py-2">
        <h2 className="text-sm font-medium">{t(drawerTitle(drawer))}</h2>
        <button
          onClick={closeDrawer}
          className="touch-target rounded px-2 text-sm text-muted hover:text-ink"
        >
          ×
        </button>
      </div>
      <div className="flex-1 overflow-y-auto p-3">
        {drawer === 'missions' && <MissionsSection />}
        {drawer === 'vehicles' && <VehiclesSection />}
        {drawer === 'layers' && <LayersSection />}
      </div>
    </aside>
  )
}

function drawerTitle(id: NonNullable<DrawerId>): string {
  switch (id) {
    case 'missions':
      return 'panels.missions'
    case 'vehicles':
      return 'panels.vehicles'
    case 'layers':
      return 'panels.layers'
  }
}

function MissionsSection() {
  const { t } = useTranslation()
  return (
    <div className="space-y-2">
      <h3 className="text-xs uppercase tracking-wide text-muted">{t('panels.tasks')}</h3>
      <div className="panel rounded p-3 text-sm text-muted">{t('views.planningPlaceholder')}</div>
      <h3 className="pt-2 text-xs uppercase tracking-wide text-muted">{t('panels.planItems')}</h3>
      <div className="space-y-1">
        {Array.from({ length: 3 }, (_, i) => (
          <div
            key={i}
            className="flex items-center justify-between rounded border border-line bg-canvas px-3 py-2 text-sm"
          >
            <span className="mono text-muted">#{i + 1}</span>
            <span className="text-ink">{t('plan.waypoint')} {i + 1}</span>
          </div>
        ))}
      </div>
    </div>
  )
}

function VehiclesSection() {
  const { t } = useTranslation()
  const link = useLinkStore((s) => s.link)
  const heartbeat = useTelemetryStore((s) => s.snapshot?.heartbeat ?? null)

  const stateLabel = fcStatusLabel(t, link)
  const tone = link?.fc_alive ? 'text-ok' : 'text-error'

  return (
    <div className="space-y-1">
      <div className="panel flex items-center justify-between rounded px-3 py-2 text-sm">
        <span>{t('panels.vehicle')}</span>
        <span className={`mono ${tone}`}>{stateLabel}</span>
      </div>
      {heartbeat && (
        <div className="panel flex items-center justify-between rounded px-3 py-2 text-sm">
          <span>Sys {heartbeat.system_id} / {heartbeat.vehicle_type}</span>
          <span className="mono text-muted">{heartbeat.autopilot}</span>
        </div>
      )}
    </div>
  )
}

function LayersSection() {
  const { t } = useTranslation()
  return (
    <div className="space-y-1">
      {[
        { key: 'terrain.dtm', on: true },
        { key: 'terrain.dsm', on: false },
        { key: 'terrain.imagery', on: false },
        { key: 'terrain.grid', on: true },
      ].map(({ key, on }) => (
        <label
          key={key}
          className="flex cursor-pointer items-center justify-between rounded border border-line bg-canvas px-3 py-2 text-sm"
        >
          <span className="text-ink">{t(key)}</span>
          <span className={`mono text-xs ${on ? 'text-ok' : 'text-muted'}`}>{on ? 'ON' : 'OFF'}</span>
        </label>
      ))}
    </div>
  )
}
