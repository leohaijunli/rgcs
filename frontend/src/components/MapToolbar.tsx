import {
  ArrowUpDown,
  Compass,
  Crosshair,
  House,
  Layers,
  LineChart,
  Move3d,
  PenTool,
  Plus,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { useTelemetryStore } from '../stores/telemetry'
import { useUiStore } from '../stores/ui'
import { useEffect, useRef, useState } from 'react'

interface Props {
  onGoHome: () => void
  /** Rotate the camera to north-up (planning map). */
  onNorth: () => void
}

export default function MapToolbar({ onGoHome, onNorth }: Props) {
  const { t } = useTranslation()
  const map3d = useUiStore((s) => s.map3d)
  const toggleMap3d = useUiStore((s) => s.toggleMap3d)
  const view = useUiStore((s) => s.view)
  const mapTool = useUiStore((s) => s.mapTool)
  const setMapTool = useUiStore((s) => s.setMapTool)
  const follow = useUiStore((s) => s.follow)
  const toggleFollow = useUiStore((s) => s.toggleFollow)
  const setQcOpen = useUiStore((s) => s.setQcOpen)
  const pos = useTelemetryStore((s) => s.snapshot?.global_position ?? null)
  const showHeights = useUiStore((s) => s.showHeights)
  const toggleHeights = useUiStore((s) => s.toggleHeights)
  const [layersOpen, setLayersOpen] = useState(false)
  const layersRef = useRef<HTMLDivElement>(null)

  // Close the Layers popover on an outside click or Esc (P1 temporary entry;
  // P3 rewrites the whole tool strip).
  useEffect(() => {
    if (!layersOpen) return
    const onDown = (e: PointerEvent) => {
      if (!layersRef.current?.contains(e.target as Node)) setLayersOpen(false)
    }
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setLayersOpen(false)
    window.addEventListener('pointerdown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('pointerdown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [layersOpen])

  return (
    // The flight-command bar owns the top-right corner in the Fly view; start
    // below it so the strips never overlap (C7).
    <div className={`absolute right-3 flex flex-col gap-1.5 rounded border border-line bg-panel/85 p-1.5 shadow-lg ${view === 'flight' ? 'top-16' : 'top-3'}`}>
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
        title={t('dock.qc')}
        active={false}
        onClick={() => setQcOpen(true)}
        icon={LineChart}
      />
      <div className="relative" ref={layersRef}>
        <ToolButton
          title={t('layers.popover')}
          active={layersOpen}
          onClick={() => setLayersOpen((o) => !o)}
          icon={Layers}
        />
        {layersOpen && <LayersPopover />}
      </div>
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

/** Layer toggles that lived in the drawer (P1 temporary home; P3 → tool strip). */
function LayersPopover() {
  const { t } = useTranslation()
  const showImagery = useUiStore((s) => s.showImagery)
  const showGrid = useUiStore((s) => s.showGrid)
  const toggleImagery = useUiStore((s) => s.toggleImagery)
  const toggleGrid = useUiStore((s) => s.toggleGrid)
  const items = [
    { key: 'terrain.imagery', on: showImagery, toggle: toggleImagery },
    { key: 'terrain.grid', on: showGrid, toggle: toggleGrid },
  ]
  return (
    <div className="absolute left-1/2 top-full z-50 mt-1 w-44 -translate-x-1/2 rounded border border-line bg-panel p-2 shadow-2xl">
      <div className="mb-1 text-[10px] uppercase tracking-wide text-muted">Layers</div>
      <div className="space-y-1">
        {items.map(({ key, on, toggle }) => (
          <button
            key={key}
            onClick={toggle}
            aria-pressed={on}
            className="flex w-full touch-target items-center justify-between rounded border border-line bg-canvas px-3 py-1.5 text-sm hover:bg-panel"
          >
            <span className="text-ink">{t(key)}</span>
            <span className={`mono text-xs ${on ? 'text-ok' : 'text-muted'}`}>
              {on ? t('layers.on') : t('layers.off')}
            </span>
          </button>
        ))}
      </div>
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