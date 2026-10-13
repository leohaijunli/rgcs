// Terrain overlay state (plan A): the dem-prep hillshade layer, loaded from
// a dem-prep output folder (N1). The folder may hold many tiles under a
// folder-level `index.json` — the store discovers them and auto-picks the
// tile covering the survey area; the drape reuses the same pick.

import { create } from 'zustand'
import type { DemBounds } from '../generated-types/DemBounds'
import type { DemSource } from '../generated-types/DemSource'

/** One tile from `terrain_discover` (absolute folder + placement). */
export interface DiscoveredTile {
  path: string
  folder: string
  bounds: DemBounds
  version: string
  has_dsm: boolean
  /** What produced the DEM — the drape report labels it (LiDAR preferred). */
  source: DemSource
}

export interface TerrainOverlay {
  /** Data URL of the hillshade PNG — null when the tile carries none
   * (raster products): the shade is optional, the DEM is what matters. */
  url: string | null
  bounds: DemBounds
  /** dem.json version (sha256[..16]) — mission archives reference it. */
  version: string
  /** True while a load is in flight. */
  loading: boolean
}

interface TerrainState {
  /** The picked dem-prep output folder (parent with index.json, or a tile). */
  folder: string | null
  /** All tiles discovered under `folder`. */
  tiles: DiscoveredTile[]
  /** The tile auto-picked for the survey area (null when uncovered). */
  tile: DiscoveredTile | null
  overlay: TerrainOverlay | null
  /** True when a survey area exists but no tile covers it (N1 hint). */
  uncovered: boolean
  /** True while a discover/load is in flight. */
  loading: boolean
  /** Error from the last load attempt (shown in the Layers popover). */
  error: string | null
  /** 3D tileset (plan B): the live tileset base URL + source folder,
   * while the loopback server runs (`null` = ellipsoid terrain). */
  terrain3d: { url: string; folder: string } | null
  /** Error from the 3D toggle / provider load (Layers popover). */
  error3d: string | null
  /** One-click build/download in flight (`fetch`/`process`/`terrain`). */
  building3d: { stage: string; line: string } | null
  /** Error from the one-click build/download. */
  buildError3d: string | null
  /** DTM under each mission waypoint (seq → metres), sampled by the map and
   * shared with the waypoint list — the AGL readouts' ground truth. */
  groundBySeq: Record<number, number> | null
  /** Re-sample the DEM under `[seq, lat, lon]` entries into `groundBySeq`. */
  sampleGround: (points: Array<[number, number, number]>) => Promise<void>
  load: () => Promise<void>
  /** Pick + load a tile against the current survey area (no dialog). */
  repick: () => Promise<void>
  /** Ensure a drapeable tile path (auto-pick; asks for a folder only when
   * nothing was loaded yet). Returns null when the area is uncovered. */
  ensureTile: () => Promise<string | null>
  clear: () => void
  /** Pick a `dem-prep terrain` folder, start the loopback tileset server
   * (plan B) and remember the base URL for the Cesium terrain provider. */
  start3d: () => Promise<void>
  /** Stop the loopback tileset server and fall back to ellipsoid terrain. */
  stop3d: () => Promise<void>
  /** Re-attach to a server that survived a webview reload
   * (`terrain3d_status`); best-effort, no dialog. */
  restore3d: () => Promise<void>
  /** The server is up but Cesium refused the tileset: clear the toggle,
   * stop the server, surface `message`. */
  fail3d: (message: string) => Promise<void>
  /** One-click Copernicus download for a lat/lon box (ADR-004 addendum):
   * fetch → process → terrain, then start serving the tileset. */
  download3d: (lat: number, lon: number, sizeKm: number) => Promise<void>
  /** One-click build from a local LiDAR/GeoTIFF folder, then start serving. */
  build3d: (folder: string) => Promise<void>
}

/**
 * Pick the tile whose bounds contain the centre of the query area (N1
 * auto-select). `query` null (no survey area yet) picks the first tile;
 * outside every tile is `null` — the caller surfaces the uncovered hint.
 */
export function pickTile<T extends { bounds: DemBounds }>(
  tiles: T[],
  query: DemBounds | null,
): T | null {
  if (tiles.length === 0) return null
  if (!query) return tiles[0]
  const cx = (query.north + query.south) / 2
  const cy = (query.east + query.west) / 2
  return (
    tiles.find(
      (t) =>
        t.bounds.north >= cx &&
        cx >= t.bounds.south &&
        t.bounds.east >= cy &&
        cy >= t.bounds.west,
    ) ?? null
  )
}

/** Bounding box of the closed draft polygon (survey area), or null. */
export async function surveyBounds(): Promise<DemBounds | null> {
  const { usePolygonStore } = await import('./polygon')
  const { vertices, closed } = usePolygonStore.getState()
  if (!closed || vertices.length < 3) return null
  const lats = vertices.map((v) => v.latitude_deg)
  const lons = vertices.map((v) => v.longitude_deg)
  return {
    north: Math.max(...lats),
    south: Math.min(...lats),
    east: Math.max(...lons),
    west: Math.min(...lons),
  }
}

export const useTerrainStore = create<TerrainState>((set, get) => ({
  folder: null,
  tiles: [],
  tile: null,
  overlay: null,
  uncovered: false,
  loading: false,
  error: null,
  terrain3d: null,
  error3d: null,
  building3d: null,
  buildError3d: null,
  groundBySeq: null,
  sampleGround: async (points) => {
    const tile = get().tile
    if (!tile || points.length === 0) return
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const samples = await invoke<Array<{ lat: number; lon: number; dtm_m: number | null }>>(
        'terrain_sample',
        { folder: tile.path, points: points.map(([, lat, lon]) => [lat, lon]) },
      )
      const next: Record<number, number> = {}
      points.forEach(([seq], i) => {
        const dtm = samples[i]?.dtm_m
        if (dtm != null) next[seq] = dtm
      })
      set({ groundBySeq: Object.keys(next).length > 0 ? next : null })
    } catch {
      set({ groundBySeq: null }) // DEM gone: fall back to the flat plane
    }
  },
  load: async () => {
    set({ error: null, loading: true })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const { open } = await import('@tauri-apps/plugin-dialog')
      const folder = await open({ multiple: false, directory: true })
      if (!folder || typeof folder !== 'string') {
        set({ loading: false })
        return
      }
      sessionStorage.setItem('maggcs.terrainFolder', folder)
      await discoverAndLoad(invoke, folder, set)
    } catch (e) {
      set({ error: String(e), loading: false })
    }
  },
  repick: async () => {
    const folder = get().folder ?? sessionStorage.getItem('maggcs.terrainFolder')
    if (!folder) return
    set({ loading: true })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await discoverAndLoad(invoke, folder, set)
    } catch (e) {
      set({ error: String(e), loading: false })
    }
  },
  ensureTile: async () => {
    const cur = get().tile
    if (cur) return cur.path
    const folder = get().folder ?? sessionStorage.getItem('maggcs.terrainFolder')
    if (folder) {
      try {
        const { invoke } = await import('@tauri-apps/api/core')
        set({ loading: true })
        await discoverAndLoad(invoke, folder, set)
        return get().tile?.path ?? null
      } catch (e) {
        set({ error: String(e), loading: false })
        return null
      }
    }
    // Nothing loaded yet: ask for the dem-prep output folder.
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const { open } = await import('@tauri-apps/plugin-dialog')
      const picked = await open({ multiple: false, directory: true })
      if (!picked || typeof picked !== 'string') return null
      sessionStorage.setItem('maggcs.terrainFolder', picked)
      set({ loading: true })
      await discoverAndLoad(invoke, picked, set)
      return get().tile?.path ?? null
    } catch (e) {
      set({ error: String(e), loading: false })
      return null
    }
  },
  clear: () => set({ overlay: null, tile: null, tiles: [], folder: null, uncovered: false, loading: false, error: null }),
  start3d: async () => {
    set({ error3d: null })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const { open } = await import('@tauri-apps/plugin-dialog')
      const folder = await open({ multiple: false, directory: true })
      if (!folder || typeof folder !== 'string') return // cancelled
      const url = await invoke<string>('terrain3d_start', { dir: folder })
      sessionStorage.setItem('maggcs.terrain3dFolder', folder)
      set({ terrain3d: { url, folder }, error3d: null })
    } catch (e) {
      set({ error3d: String(e) })
    }
  },
  stop3d: async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('terrain3d_stop')
    } catch (e) {
      // Keep the toggle on — the server may still be running.
      set({ error3d: String(e) })
      return
    }
    set({ terrain3d: null, error3d: null })
  },
  restore3d: async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const url = await invoke<string | null>('terrain3d_status')
      if (!url) return
      const folder = sessionStorage.getItem('maggcs.terrain3dFolder') ?? ''
      set({ terrain3d: { url, folder } })
    } catch {
      /* best-effort: the toggle simply stays off */
    }
  },
  fail3d: async (message: string) => {
    set({ terrain3d: null, error3d: message })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('terrain3d_stop')
    } catch {
      /* server already gone */
    }
  },
  download3d: async (lat, lon, sizeKm) => {
    startBuildWatcher()
    set({ building3d: { stage: 'fetch', line: '' }, buildError3d: null })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const out = await invoke<string>('terrain3d_download', {
        lat,
        lon,
        sizeKm,
        maxzoom: null,
        outDir: await defaultOutDir('glo30'),
      })
      await serveBuilt3d(out)
      set({ building3d: null })
    } catch (e) {
      set({ building3d: null, buildError3d: String(e) })
    }
  },
  build3d: async (folder) => {
    startBuildWatcher()
    set({ building3d: { stage: 'process', line: '' }, buildError3d: null })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const out = await invoke<string>('terrain3d_build', { folder, maxzoom: null })
      await serveBuilt3d(out)
      set({ building3d: null })
    } catch (e) {
      set({ building3d: null, buildError3d: String(e) })
    }
  },
}))

/** Output root for the one-click download: the app data dir (or the CWD in
 * browser dev) + a fixed name, so repeat runs reuse the cached tiles. */
async function defaultOutDir(name: string): Promise<string> {
  try {
    const { appDataDir, join } = await import('@tauri-apps/api/path')
    return await join(await appDataDir(), name)
  } catch {
    return name
  }
}

/** Serve a freshly built tileset: start the loopback server on it and flip
 * the toggle on (the same state `start3d` would produce, minus the dialog). */
async function serveBuilt3d(tileset: string): Promise<void> {
  const { invoke } = await import('@tauri-apps/api/core')
  const url = await invoke<string>('terrain3d_start', { dir: tileset })
  sessionStorage.setItem('maggcs.terrain3dFolder', tileset)
  useTerrainStore.setState({
    terrain3d: { url, folder: tileset },
    error3d: null,
  })
}

let buildWatcherStarted = false

/** Stream `terrain3d_progress` events ({stage, line}) into `building3d` —
 * the one-click pipeline's per-line log, started lazily and once. */
function startBuildWatcher(): void {
  if (buildWatcherStarted) return
  buildWatcherStarted = true
  void (async () => {
    try {
      const { listen } = await import('@tauri-apps/api/event')
      await listen<{ stage: string; line: string }>('terrain3d_progress', (e) => {
        useTerrainStore.setState({ building3d: e.payload })
      })
    } catch {
      /* events unavailable (browser dev) */
    }
  })()
}

type Setter = (partial: Partial<TerrainState>) => void

/** Discover tiles under `folder`, auto-pick by survey area, load overlay.
 * A folder that carries neither `index.json` nor `dem.json` is treated as
 * **raw** BC LiDAR downloads (one subfolder per grid tile): it is processed
 * once through `dem-prep process` (recursive, `<source>-processed`) and the
 * generated folder is discovered instead. */
async function discoverAndLoad(
  invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>,
  folder: string,
  set: Setter,
): Promise<void> {
  let effective = folder
  let tiles: DiscoveredTile[]
  if (await hasDemIndex(invoke, folder)) {
    ;({ tiles } = await invoke<{ tiles: DiscoveredTile[] }>('terrain_discover', {
      folder: effective,
    }))
  } else {
    // Raw BC LiDAR downloads: one subfolder per grid tile. Processed once
    // (recursively), then the generated folder is used from here on.
    set({ loading: true })
    const raw = await invoke<{ processed: string; tiles: DiscoveredTile[] }>(
      'terrain_process_raw',
      { folder },
    )
    effective = raw.processed
    tiles = raw.tiles
    if (tiles.length === 0) {
      set({ loading: false, error: 'no DEM tiles produced from ' + folder })
      return
    }
  }
  const bounds = await surveyBounds()
  const tile = pickTile(tiles, bounds)
  set({ folder: effective, tiles, tile, uncovered: tiles.length > 0 && tile == null })
  if (!tile) {
    set({ overlay: null, loading: false })
    return
  }
  const overlay = await invoke<Omit<TerrainOverlay, 'loading'>>('terrain_load', {
    folder: tile.path,
  })
  // Keep the drape (and the next boot) pointed at the tile that was loaded.
  sessionStorage.setItem('maggcs.terrainFolder', tile.path)
  set({ overlay: { ...overlay, loading: false }, error: null, loading: false })
}

/** Whether `folder` is already a processed dem-prep output: simply probe
 * `terrain_discover` (it accepts a parent with index.json or a single-tile
 * folder with dem.json). No fs-plugin reads — the desktop capability only
 * grants text-file reads, so a readDir probe silently failed and misrouted
 * processed folders back into dem-prep (bug: "no DEM tiles produced"). */
async function hasDemIndex(
  invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>,
  folder: string,
): Promise<boolean> {
  try {
    const { tiles } = await invoke<{ tiles: DiscoveredTile[] }>('terrain_discover', {
      folder,
    })
    return tiles.length > 0
  } catch {
    return false
  }
}
