import {
  ArrowUpDown,
  Compass,
  Crosshair,
  Gauge,
  House,
  LineChart,
  Move3d,
  PenTool,
  Plus,
  Ruler,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore } from '../stores/ui'

interface Props {
  onGoHome: () => void
  onToggleMeasure: () => void
  /** Rotate the camera to north-up (planning map). */
  onNorth: () => void
}

export default function MapToolbar({ onGoHome, onToggleMeasure, onNorth }: Props) {
  const { t } = useTranslation()
  const map3d = useUiStore((s) => s.map3d)
  const toggleMap3d = useUiStore((s) => s.toggleMap3d)
  const view = useUiStore((s) => s.view)
  const mapTool = useUiStore((s) => s.mapTool)
  const setMapTool = useUiStore((s) => s.setMapTool)
  const follow = useUiStore((s) => s.follow)
  const toggleFollow = useUiStore((s) => s.toggleFollow)
  const setDashboardOpen = useUiStore((s) => s.setDashboardOpen)
  const setQcOpen = useUiStore((s) => s.setQcOpen)
  const pos = useTelemetryStore((s) => s.snapshot?.global_position ?? null)
  const showHeights = useUiStore((s) => s.showHeights)
  const toggleHeights = useUiStore((s) => s.toggleHeights)

  return (
    <div className="absolute right-3 top-3 flex flex-col gap-1.5 rounded border border-line bg-panel/85 p-1.5 shadow-lg">
      {view === 'planning' && (
        <ToolButton
          title={t('map.addWaypoint')}
          active={mapTool === 'add'}
          onClick={() => setMapTool(mapTool === 'add' ? 'select' : 'add')}
          icon={Plus}
        />
      )}
      {view === 'planning' && (
        <ToolButton
          title={t('map.drawPolygon')}
          active={mapTool === 'polygon'}
          onClick={() => setMapTool(mapTool === 'polygon' ? 'select' : 'polygon')}
          icon={PenTool}
        />
      )}
      <ToolButton
        title={t('map.dashboard')}
        active={false}
        onClick={() => setDashboardOpen(true)}
        icon={Gauge}
      />
      <ToolButton
        title={t('dock.qc')}
        active={false}
        onClick={() => setQcOpen(true)}
        icon={LineChart}
      />
      <ToolButton
        title={t('map.mode3d')}
        active={map3d}
        onClick={toggleMap3d}
        icon={Move3d}
      />
      <ToolButton
        title={t('map.heights')}
        active={showHeights}
        onClick={toggleHeights}
        icon={ArrowUpDown}
      />
      <ToolButton title={t('map.follow')} active={follow} onClick={toggleFollow} icon={Crosshair} />
      <ToolButton title={t('map.home')} onClick={onGoHome} icon={House} />
      <ToolButton title={t('map.measure')} onClick={onToggleMeasure} icon={Ruler} />
      <ToolButton title={t('map.north')} onClick={onNorth} icon={Compass} />
      {pos && (
        <div className="mono border-t border-line px-2 py-1 text-[10px] leading-tight text-muted">
          {pos.latitude_deg.toFixed(5)}
          <br />
          {pos.longitude_deg.toFixed(5)}
          <br />
          {Math.round(pos.altitude.meters)} m
        </div>
      )}
    </div>
  )
}

function ToolButton({
  title,
  onClick,
  icon: Icon,
  active,
}: {
  title: string
  onClick: () => void
  icon: typeof Crosshair
  active?: boolean
}) {
  return (
    <button
      title={title}
      aria-label={title}
      onClick={onClick}
      className={`flex h-11 w-11 items-center justify-center rounded transition-colors ${
        active ? 'bg-accent text-accent-ink' : 'text-muted hover:bg-canvas hover:text-ink'
      }`}
    >
      <Icon size={17} strokeWidth={1.75} />
    </button>
  )
}