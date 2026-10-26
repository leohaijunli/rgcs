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
import { invoke } from '@tauri-apps/api/core'
import { getSafetyConfig } from '../desktop/appConfig'
import { TaskProgress } from '../design-system/components'
import { useTerrainStore } from '../stores/terrain'
import {
  IconButton,
  Popover,
  PopoverAction,
  PopoverSection,
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
  const showGrid = useUiStore((s) => s.showGrid)
  const imageryStyle = useUiStore((s) => s.imageryStyle)
  const setImageryStyle = useUiStore((s) => s.setImageryStyle)
  const toggleGrid = useUiStore((s) => s.toggleGrid)
  const showHeights = useUiStore((s) => s.showHeights)
  const toggleHeights = useUiStore((s) => s.toggleHeights)
  const showCoverage = useUiStore((s) => s.showCoverage)
  const toggleCoverage = useUiStore((s) => s.toggleCoverage)
  const hillshadeOpacity = useUiStore((s) => s.hillshadeOpacity)
  const setHillshadeOpacity = useUiStore((s) => s.setHillshadeOpacity)
  const resetLayers = useUiStore((s) => s.resetLayers)
  const hasCoverage = useTerrainStore((s) => s.coverage != null)
  const overlay = useTerrainStore((s) => s.overlay)
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

  const isPlanning = view === 'plan'

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
      {isPlanning && (
        <IconButton
          title={`${t('map.setCenter')} (C)`}
          active={mapTool === 'center'}
          onClick={() => setMapTool(mapTool === 'center' ? 'select' : 'center')}
        >
          <Crosshair size={17} strokeWidth={1.75} />
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
        // U16: grouped by what the layer *is* (base map / terrain / mission /
        // vehicle), each row carrying its own state — an unloaded or failed
        // layer never reads "on". Vehicle position and alerts are NOT
        // toggleable: a safety readout must not be silently hidden.
        <Popover className="left-full top-0 ml-1.5 w-56">
          <PopoverSection label={t('layers.baseMap')} first />
          <div className="flex rounded border border-line bg-canvas p-0.5">
            {(['google-satellite', 'google-hybrid'] as const).map((style) => (
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

          <PopoverSection label={t('layers.terrain')} />
          <TerrainToggle />
          {overlay && (
            <label className="flex items-center gap-2 px-1 text-[10px] text-muted" data-testid="hillshade-opacity">
              <span className="shrink-0">{t('layers.opacity')}</span>
              <input
                type="range"
                min={0.2}
                max={1}
                step={0.05}
                value={hillshadeOpacity}
                onChange={(e) => setHillshadeOpacity(Number(e.target.value))}
                className="h-1 flex-1"
              />
              <span className="mono w-8 text-right">{Math.round(hillshadeOpacity * 100)}%</span>
            </label>
          )}
          <Terrain3DToggle />

          <PopoverSection label={t('layers.mission')} />
          {hasCoverage && (
            <PopoverToggle label={t('map.coverage')} on={showCoverage} onLabel={t('layers.on')} offLabel={t('layers.off')} onToggle={toggleCoverage} />
          )}
          <PopoverToggle label={t('map.heights')} on={showHeights} onLabel={t('layers.on')} offLabel={t('layers.off')} onToggle={toggleHeights} />
          <div className="px-1 text-[10px] text-muted">{t('layers.vehicleAlwaysOn')}</div>

          <button
            data-testid="layers-reset"
            onClick={resetLayers}
            className="mt-1 w-full rounded border border-line bg-canvas px-2 py-1 text-[11px] text-muted hover:bg-panel hover:text-ink"
          >
            {t('layers.reset')}
          </button>
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
 * runs and the globe carries the mesh. The DEM is taken from the local DEM
 * folder when one is present, else from the survey area's HRDEM session
 * (fetched once, then cached); the pipeline's progress streams below. */
function Terrain3DToggle() {
  const { t } = useTranslation()
  const terrain3d = useTerrainStore((s) => s.terrain3d)
  const error3d = useTerrainStore((s) => s.error3d)
  const building3d = useTerrainStore((s) => s.building3d)
  const buildError3d = useTerrainStore((s) => s.buildError3d)
  const start3d = useTerrainStore((s) => s.start3d)
  const stop3d = useTerrainStore((s) => s.stop3d)
  const folderName = terrain3d
    ? terrain3d.folder.split(/[\\/]/).filter(Boolean).pop() ?? terrain3d.folder
    : null

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
      {!terrain3d && building3d && (
        // U13: the 3D build gets the shared readout — stage, streamed log
        // line, elapsed, and the cancel that kills the dem-prep child.
        <div className="px-1">
          <TaskProgress
            label={`[${building3d.stage}] ${(building3d.line.slice(-40)) || t('terrain.building')}`}
            indeterminate
            startedAtMs={building3d.startedAtMs}
            danger={false}
            onCancel={() => void invoke<boolean>('terrain3d_cancel').catch(() => {})}
          />
        </div>
      )}
      {!terrain3d && !building3d && (
        <div className="px-1 text-[10px] text-muted">{t('terrain.autoHint')}</div>
      )}
      {buildError3d && <div className="px-1 text-[10px] text-error">{buildError3d}</div>}
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
  const suggestDownload = useTerrainStore((s) => s.suggestDownload)
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
      {overlay && tile && (
        <div className="px-1 text-[10px] text-muted" data-testid="terrain-chip">
          {/* H6 chip: source / resolution / acquisition age / datum, with the
           * age warning when past the configured threshold. */}
          {tile.source === 'hrdem'
            ? t('terrain.sourceHrdem')
            : tile.source === 'geotiff'
              ? t('terrain.sourceGeotiff')
              : t('terrain.sourceLidar')}
          {tile.resolution_m != null && <span> · {tile.resolution_m} m</span>}
          {tile.acquisition && (
            <DemAgeChip acquisition={tile.acquisition} />
          )}
          {tile.vertical_datum && (
            <span>
              {' · '}
              {tile.vertical_datum === 'orthometric_cgvd2013'
                ? t('terrain.datumCgvd2013')
                : t('terrain.datumEgm96')}
            </span>
          )}
          <span>
            {' · '}
            {t('terrain.version')} {overlay.version}
          </span>
          {tiles.length > 1 && (
            <span>
              {' · '}
              {tile.folder || '—'} ({tiles.length})
            </span>
          )}
        </div>
      )}
      {suggestDownload && !error && (
        <div className="px-1 text-[10px] text-warn">{t('terrain.suggestDownload')}</div>
      )}
    </div>
  )
}


/** H6: the acquisition age with the configured staleness warning. */
function DemAgeChip({ acquisition }: { acquisition: string }) {
  const { t } = useTranslation()
  const years = (Date.now() - Date.parse(acquisition)) / (365.25 * 24 * 3600 * 1000)
  if (!Number.isFinite(years)) return null
  const limit = getSafetyConfig().dem_max_age_years
  const stale = limit != null && years > limit
  return (
    <span className={stale ? 'text-warn' : undefined}>
      {' · '}
      {t('terrain.acquired', { year: acquisition.slice(0, 4) })}
      {stale ? ' ⚠' : ''}
    </span>
  )
}
