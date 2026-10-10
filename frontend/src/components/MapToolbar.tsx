// Map tool strip (P3 §4.3): a vertical strip in the map's top-left corner —
// the right side belongs to the flight commands and the right panel, so the
// two can no longer collide (C7). One row of edit tools per view, everything
// else folded into two grouped popovers: Layers ▸ and View ▸.
//   Plan: Select · Add waypoint (W) · Draw polygon (P) · Layers ▸ · View ▸
//   Fly:  Follow · 2D/3D · Layers ▸ · View ▸

import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  Compass,
  Crosshair,
  House,
  Layers,
  MoreVertical,
  MousePointer2,
  Move3d,
  PenTool,
  Plus,
} from 'lucide-react'
import { useUiStore } from '../stores/ui'

interface Props {
  onGoHome: () => void
  /** Rotate the camera to north-up. */
  onNorth: () => void
}

export default function MapToolbar({ onGoHome, onNorth }: Props) {
  const { t } = useTranslation()
  const view = useUiStore((s) => s.view)
  const mapTool = useUiStore((s) => s.mapTool)
  const setMapTool = useUiStore((s) => s.setMapTool)
  const follow = useUiStore((s) => s.follow)
  const toggleFollow = useUiStore((s) => s.toggleFollow)
  const map3d = useUiStore((s) => s.map3d)
  const toggleMap3d = useUiStore((s) => s.toggleMap3d)
  const showImagery = useUiStore((s) => s.showImagery)
  const toggleImagery = useUiStore((s) => s.toggleImagery)
  const showGrid = useUiStore((s) => s.showGrid)
  const toggleGrid = useUiStore((s) => s.toggleGrid)
  const showHeights = useUiStore((s) => s.showHeights)
  const toggleHeights = useUiStore((s) => s.toggleHeights)
  const [popover, setPopover] = useState<'layers' | 'view' | null>(null)
  const rootRef = useRef<HTMLDivElement>(null)

  // One popover at a time; close on outside click or Esc.
  useEffect(() => {
    if (!popover) return
    const onDown = (e: PointerEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setPopover(null)
    }
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setPopover(null)
    window.addEventListener('pointerdown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('pointerdown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [popover])

  const isPlanning = view === 'planning'

  return (
    <div
      ref={rootRef}
      className="absolute left-3 top-3 z-10 flex flex-col gap-1.5 rounded border border-line bg-panel/85 p-1.5 shadow-lg"
    >
      {isPlanning && (
        <ToolButton
          title={`${t('map.select')} (Esc)`}
          active={mapTool === 'select'}
          onClick={() => setMapTool('select')}
          icon={MousePointer2}
        />
      )}
      {isPlanning && (
        <ToolButton
          title={`${t('map.addWaypoint')} (W)`}
          active={mapTool === 'add'}
          onClick={() => setMapTool(mapTool === 'add' ? 'select' : 'add')}
          icon={Plus}
        />
      )}
      {isPlanning && (
        <ToolButton
          title={`${t('map.drawPolygon')} (P)`}
          active={mapTool === 'polygon'}
          onClick={() => setMapTool(mapTool === 'polygon' ? 'select' : 'polygon')}
          icon={PenTool}
        />
      )}
      {!isPlanning && (
        <ToolButton
          title={t('map.follow')}
          active={follow}
          onClick={toggleFollow}
          icon={Crosshair}
        />
      )}
      {!isPlanning && (
        <ToolButton title={t('map.mode3d')} active={map3d} onClick={toggleMap3d} icon={Move3d} />
      )}
      <ToolButton
        title={t('layers.popover')}
        active={popover === 'layers'}
        onClick={() => setPopover((p) => (p === 'layers' ? null : 'layers'))}
        icon={Layers}
      />
      <ToolButton
        title={t('view.popover')}
        active={popover === 'view'}
        onClick={() => setPopover((p) => (p === 'view' ? null : 'view'))}
        icon={MoreVertical}
      />

      {popover === 'layers' && (
        <Popover>
          <PopoverToggle label={t('terrain.imagery')} on={showImagery} onToggle={toggleImagery} />
          <PopoverToggle label={t('terrain.grid')} on={showGrid} onToggle={toggleGrid} />
          <PopoverToggle label={t('map.heights')} on={showHeights} onToggle={toggleHeights} />
        </Popover>
      )}
      {popover === 'view' && (
        <Popover>
          {isPlanning && (
            <PopoverToggle label={t('map.mode3d')} on={map3d} onToggle={toggleMap3d} />
          )}
          <PopoverAction label={t('map.north')} icon={Compass} onClick={onNorth} />
          <PopoverAction label={t('map.home')} icon={House} onClick={onGoHome} />
        </Popover>
      )}
    </div>
  )
}

/** Popover panel anchored to the strip's right edge. */
function Popover({ children }: { children: React.ReactNode }) {
  return (
    <div className="absolute left-full top-0 z-50 ml-1.5 w-48 rounded border border-line bg-panel p-1.5 shadow-2xl">
      <div className="space-y-1">{children}</div>
    </div>
  )
}

function PopoverToggle({
  label,
  on,
  onToggle,
}: {
  label: string
  on: boolean
  onToggle: () => void
}) {
  const { t } = useTranslation()
  return (
    <button
      onClick={onToggle}
      aria-pressed={on}
      className="flex w-full touch-target items-center justify-between rounded border border-line bg-canvas px-3 py-1.5 text-sm text-ink hover:bg-panel"
    >
      <span className="truncate">{label}</span>
      <span className={`mono text-xs ${on ? 'text-ok' : 'text-muted'}`}>
        {on ? t('layers.on') : t('layers.off')}
      </span>
    </button>
  )
}

function PopoverAction({
  label,
  icon: Icon,
  onClick,
}: {
  label: string
  icon: typeof Compass
  onClick: () => void
}) {
  return (
    <button
      onClick={onClick}
      className="flex w-full touch-target items-center gap-2 rounded border border-line bg-canvas px-3 py-1.5 text-sm text-ink hover:bg-panel"
    >
      <Icon size={14} strokeWidth={1.75} />
      <span className="truncate">{label}</span>
    </button>
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