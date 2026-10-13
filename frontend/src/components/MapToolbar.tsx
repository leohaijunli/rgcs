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
import { useTerrainStore } from '../stores/terrain'
import {
  IconButton,
  Popover,
  PopoverAction,
  PopoverToggle,
} from '../design-system/components'

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
  const imageryStyle = useUiStore((s) => s.imageryStyle)
  const setImageryStyle = useUiStore((s) => s.setImageryStyle)
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
        <IconButton
          title={`${t('map.select')} (Esc)`}
          active={mapTool === 'select'}
          onClick={() => setMapTool('select')}
        >
          <MousePointer2 size={17} strokeWidth={1.75} />
        </IconButton>
      )}
      {isPlanning && (
        <IconButton
          title={`${t('map.addWaypoint')} (W)`}
          active={mapTool === 'add'}
          onClick={() => setMapTool(mapTool === 'add' ? 'select' : 'add')}
        >
          <Plus size={17} strokeWidth={1.75} />
        </IconButton>
      )}
      {isPlanning && (
        <IconButton
          title={`${t('map.drawPolygon')} (P)`}
          active={mapTool === 'polygon'}
          onClick={() => setMapTool(mapTool === 'polygon' ? 'select' : 'polygon')}
        >
          <PenTool size={17} strokeWidth={1.75} />
        </IconButton>
      )}
      {!isPlanning && (
        <IconButton title={t('map.follow')} active={follow} onClick={toggleFollow}>
          <Crosshair size={17} strokeWidth={1.75} />
        </IconButton>
      )}
      {!isPlanning && (
        <IconButton title={t('map.mode3d')} active={map3d} onClick={toggleMap3d}>
          <Move3d size={17} strokeWidth={1.75} />
        </IconButton>
      )}
      <IconButton
        title={t('layers.popover')}
        active={popover === 'layers'}
        onClick={() => setPopover((p) => (p === 'layers' ? null : 'layers'))}
      >
        <Layers size={17} strokeWidth={1.75} />
      </IconButton>
      <IconButton
        title={t('view.popover')}
        active={popover === 'view'}
        onClick={() => setPopover((p) => (p === 'view' ? null : 'view'))}
      >
        <MoreVertical size={17} strokeWidth={1.75} />
      </IconButton>

      {popover === 'layers' && (
        <Popover className="left-full top-0 ml-1.5 w-48">
          <div className="flex rounded border border-line bg-canvas p-0.5">
            {(['osm', 'satellite', 'google', 'topo'] as const).map((style) => (
              <button
                key={style}
                className={`flex-1 rounded px-1.5 py-1 text-[11px] transition-colors ${
                  imageryStyle === style
                    ? 'bg-accent text-accent-ink'
                    : 'text-muted hover:text-ink'
                }`}
                onClick={() => setImageryStyle(style)}
              >
                {t(`terrain.${style}`)}
              </button>
            ))}
          </div>
          <PopoverToggle label={t('terrain.grid')} on={showGrid} onLabel={t('layers.on')} offLabel={t('layers.off')} onToggle={toggleGrid} />
          <PopoverToggle label={t('map.heights')} on={showHeights} onLabel={t('layers.on')} offLabel={t('layers.off')} onToggle={toggleHeights} />
          <TerrainToggle />
          <Terrain3DToggle />
        </Popover>
      )}
      {popover === 'view' && (
        <Popover className="left-full top-0 ml-1.5 w-48">
          {isPlanning && (
            <PopoverToggle label={t('map.mode3d')} on={map3d} onLabel={t('layers.on')} offLabel={t('layers.off')} onToggle={toggleMap3d} />
          )}
          <PopoverAction label={t('map.north')} icon={<Compass size={14} strokeWidth={1.75} />} onClick={onNorth} />
          <PopoverAction label={t('map.home')} icon={<House size={14} strokeWidth={1.75} />} onClick={onGoHome} />
        </Popover>
      )}
    </div>
  )
}

/** 3D terrain mesh (plan B): serve a `dem-prep terrain` tileset from the
 * loopback HTTP server and hand Cesium the layer.json URL; on = the server
 * runs and the globe carries the mesh. Below the toggle: the two one-click
 * sources (Copernicus download for the map centre, or a local LiDAR folder)
 * with the pipeline's progress lines. */
function Terrain3DToggle() {
  const { t } = useTranslation()
  const terrain3d = useTerrainStore((s) => s.terrain3d)
  const error3d = useTerrainStore((s) => s.error3d)
  const building3d = useTerrainStore((s) => s.building3d)
  const buildError3d = useTerrainStore((s) => s.buildError3d)
  const start3d = useTerrainStore((s) => s.start3d)
  const stop3d = useTerrainStore((s) => s.stop3d)
  const download3d = useTerrainStore((s) => s.download3d)
  const build3d = useTerrainStore((s) => s.build3d)
  const mapCenter = useUiStore((s) => s.mapCenter)
  const folderName = terrain3d
    ? terrain3d.folder.split(/[\\/]/).filter(Boolean).pop() ?? terrain3d.folder
    : null

  const downloadHere = () => {
    // The map centre at click time; without one (rare) fall back to the
    // Chilliwack drape-verification area.
    const c = mapCenter ?? { lat: 49.1577, lon: -121.951 }
    void download3d(c.lat, c.lon, 50)
  }

  const buildLocal = async () => {
    const { open } = await import('@tauri-apps/plugin-dialog')
    const folder = await open({ multiple: false, directory: true })
    if (folder && typeof folder === 'string') void build3d(folder)
  }

  return (
    <div>
      <PopoverToggle
        label={t('terrain.threeD')}
        on={terrain3d != null}
        onLabel={t('layers.on')}
        offLabel={t('layers.off')}
        onToggle={() => (terrain3d ? void stop3d() : void start3d())}
      />
      {error3d && <div className="px-1 text-[10px] text-error">{error3d}</div>}
      {folderName && (
        <div className="px-1 text-[10px] text-muted">
          {t('terrain.serving')} {folderName}
        </div>
      )}
      {!terrain3d && (
        <div className="mt-1 space-y-1 border-t border-line pt-1">
          <button
            onClick={downloadHere}
            disabled={building3d != null || !mapCenter}
            title={t('terrain.downloadHint')}
            className="w-full rounded border border-line bg-canvas px-2 py-1 text-[11px] text-ink hover:bg-panel disabled:opacity-50"
          >
            {t('terrain.downloadHere')}
          </button>
          <button
            onClick={() => void buildLocal()}
            disabled={building3d != null}
            title={t('terrain.buildHint')}
            className="w-full rounded border border-line bg-canvas px-2 py-1 text-[11px] text-ink hover:bg-panel disabled:opacity-50"
          >
            {t('terrain.buildLocal')}
          </button>
          {building3d && (
            <div className="mono px-1 text-[10px] text-muted">
              [{building3d.stage}] {building3d.line.slice(-60)}
            </div>
          )}
          {buildError3d && <div className="px-1 text-[10px] text-error">{buildError3d}</div>}
        </div>
      )}
    </div>
  )
}

/** Terrain hillshade overlay (plan A): load a dem-prep tile folder from the
 * Layers popover; on = an overlay is loaded. */
function TerrainToggle() {
  const { t } = useTranslation()
  const overlay = useTerrainStore((s) => s.overlay)
  const error = useTerrainStore((s) => s.error)
  const uncovered = useTerrainStore((s) => s.uncovered)
  const tiles = useTerrainStore((s) => s.tiles)
  const tile = useTerrainStore((s) => s.tile)
  const load = useTerrainStore((s) => s.load)
  const clear = useTerrainStore((s) => s.clear)
  return (
    <div>
      <PopoverToggle
        label={t('terrain.overlay')}
        on={overlay != null}
        onLabel={t('layers.on')}
        offLabel={t('layers.off')}
        onToggle={() => (overlay ? clear() : void load())}
      />
      {error && <div className="px-1 text-[10px] text-error">{error}</div>}
      {uncovered && !error && (
        <div className="px-1 text-[10px] text-warn">{t('terrain.uncovered')}</div>
      )}
      {overlay && (
        <div className="px-1 text-[10px] text-muted">
          {t('terrain.version')} {overlay.version}
          {tiles.length > 1 && tile && (
            <span>
              {' · '}
              {tile.folder || '—'} ({tiles.length})
            </span>
          )}
        </div>
      )}
    </div>
  )
}
