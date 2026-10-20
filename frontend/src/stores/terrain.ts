// Terrain overlay state (plan A): the dem-prep hillshade layer, loaded from
// a dem-prep output folder (N1). The folder may hold many tiles under a
// folder-level `index.json` — the store discovers them and auto-picks the
// tile covering the survey area; the drape reuses the same pick.

import { create } from 'zustand'
import type { DemBounds } from '../generated-types/DemBounds'
import type { DemSource } from '../generated-types/DemSource'
import type { SurveyPrepStatus } from '../generated-types/SurveyPrepStatus'
import { firstSurface, type SurfaceSource } from '../util/surface'

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

/** A live 3D mesh the frontend can sample: published by the map while the
 * Cesium terrain provider holds the tileset (the backend cannot read it). */
export interface MeshSampler {
  /** Raw tileset heights under `[lat, lon]` points, `null` outside coverage. */
  sample: (points: Array<[number, number]>) => Promise<Array<number | null>>
  /** The tileset's vertical datum: `ellipsoidWgs84` heights are `h` (AMSL =
   * `h − N(EGM96)`), `orthometric` heights are CGVD2013 `H` (≈ AMSL). */
  verticalDatum: 'ellipsoidWgs84' | 'orthometric'
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
  /** U9b: the in-flight "prepare survey area" line (stage `hrdem`), or
   * `null` when idle. Kept apart from `building3d` so the two pipelines can
   * stream at once without their logs clobbering each other. */
  surveyPrep: { stage: string; line: string } | null
  /** U9b: the prepared DEM footprint to outline on the map, or `null`. */
  coverage: DemBounds | null
  setCoverage: (bounds: DemBounds | null) => void
  /** One-click build/download in flight (`fetch`/`process`/`terrain`). */
  building3d: { stage: string; line: string } | null
  /** Error from the one-click build/download. */
  buildError3d: string | null
  /** DTM under each mission waypoint (seq → metres), sampled by the map and
   * shared with the waypoint list — the AGL readouts' ground truth. */
  groundBySeq: Record<number, number> | null
  /** DEM/mesh ground under the FC HOME (the arm point, AMSL) — anchors the
   * live vehicle into the plan's frame (`vehicleAmslAnchoredM`); null when no
   * terrain or no HOME fix. */
  homeGroundAmsl: number | null
  /** Landed self-calibration (2026-10-18 field report): while the vehicle
   * reports ON_GROUND with a known ground sample, `offset = ground −
   * reported AMSL`. The altimeter's anchor error is a constant bias, so this
   * offset fixes the whole flight — including landing at a different
   * elevation than takeoff (the DEM under the current position cancels it).
   * Preferred over the HOME-ground path, which needs a HOME_POSITION fix
   * PX4 does not always stream. */
  amslAnchorOffsetM: number | null
  /** The live 3D mesh sampler (see [`MeshSampler`]), or `null` when no mesh
   * is active. The terrain drape prefers it over the 2D tile. */
  meshSampler: MeshSampler | null
  /** Re-sample the DEM under `[seq, lat, lon]` entries into `groundBySeq`. */
  sampleGround: (points: Array<[number, number, number]>) => Promise<void>
  /** DTM under `[lat, lon]` points (metres, the DEM's own datum), sampled
   * from the loaded 2D tile. `null` outside coverage — the caller falls back
   * to the mesh or the HOME plane. Does not touch state. */
  sampleDem: (points: Array<[number, number]>) => Promise<Array<number | null>>
  /** Ground (AMSL) under one point: the live 3D mesh first (converting an
   * ellipsoid tileset with N(EGM96)), then the loaded DEM tile, else `null`.
   * The flight commands that need a terrain target (land, set home) use this
   * so they follow the same reference as every AGL readout. */
  samplePointAmsl: (lat: number, lon: number) => Promise<number | null>
  load: () => Promise<void>
  /** Pick + load a tile against the current survey area (no dialog). */
  repick: () => Promise<void>
  /** Ensure a drapeable tile path with no dialog when possible: a remembered
   * dem-prep folder, the survey area's prepared HRDEM session, or an
   * auto-detected DEM folder. With `allowDialog` (default) the folder picker
   * is the last resort; automatic callers pass `false` so a background drape
   * never surprises the operator with a dialog. Returns null when nothing is
   * available. */
  ensureTile: (allowDialog?: boolean) => Promise<string | null>
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
  /** U9b: fetch the HRDEM DTM+DSM for an AOI. Resolves to the resulting
   * readiness status, or `null` when cancelled; rejects with a stringified
   * backend error (uncovered / cross-project / offline / missing GDAL). */
  prepareSurvey: (
    lat: number,
    lon: number,
    width: number,
    height: number,
  ) => Promise<SurveyPrepStatus | null>
  /** U9b: cancel an in-flight prepare (kills the dem-prep child). */
  cancelSurvey: () => Promise<void>
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
  surveyPrep: null,
  coverage: null,
  setCoverage: (coverage) => set({ coverage }),
  building3d: null,
  buildError3d: null,
  groundBySeq: null,
  homeGroundAmsl: null,
  amslAnchorOffsetM: null,
  meshSampler: null,
  sampleGround: async (points) => {
    if (points.length === 0) {
      set({ groundBySeq: null })
      return
    }
    const heights = await get().sampleDem(points.map(([, lat, lon]) => [lat, lon]))
    const next: Record<number, number> = {}
    points.forEach(([seq], i) => {
      const dtm = heights[i]
      if (dtm != null) next[seq] = dtm
    })
    set({ groundBySeq: Object.keys(next).length > 0 ? next : null })
  },
  samplePointAmsl: async (lat, lon) => {
    const mesh = get().meshSampler
    if (mesh) {
      try {
        const [h] = await mesh.sample([[lat, lon]])
        if (h != null && Number.isFinite(h)) {
          if (mesh.verticalDatum === 'orthometric') return h
          const { invoke } = await import('@tauri-apps/api/core')
          const samples = await invoke<Array<{ undulation_m: number | null }>>('geoid_sample', {
            points: [[lat, lon]],
          })
          const n = samples[0]?.undulation_m
          // No geoid: an ellipsoid height cannot become AMSL — let the DEM
          // tile answer instead of returning a height that is ~N metres off.
          if (n != null && Number.isFinite(n)) return h - n
        }
      } catch {
        /* mesh gone: fall through to the DEM tile */
      }
    }
    const [dtm] = await get().sampleDem([[lat, lon]])
    return dtm ?? null
  },
  sampleDem: async (points) => {
    const tile = get().tile
    if (!tile || points.length === 0) return points.map(() => null)
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const samples = await invoke<Array<{ lat: number; lon: number; dtm_m: number | null }>>(
        'terrain_sample',
        { folder: tile.path, points },
      )
      return points.map((_, i) => samples[i]?.dtm_m ?? null)
    } catch {
      return points.map(() => null) // DEM gone: fall back to the flat plane
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
  ensureTile: async (allowDialog = true) => {
    const cur = get().tile
    if (cur) return cur.path
    const invoke = async <T,>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
      const { invoke: call } = await import('@tauri-apps/api/core')
      return call<T>(cmd, args)
    }
    // Load one candidate folder and report the tile it yielded (if any). A
    // failure is recorded but not thrown: `firstSurface` moves on to the next
    // source, and a later success clears the stale error.
    const load = async (folder: string): Promise<string | null> => {
      set({ loading: true })
      try {
        await discoverAndLoad(invoke, folder, set)
      } catch (e) {
        set({ error: String(e), loading: false })
        return null
      }
      const path = get().tile?.path ?? null
      if (path) set({ error: null })
      return path
    }
    // Priority order (the folder picker is the last resort): a remembered
    // dem-prep folder → the survey area's prepared HRDEM session → a DEM
    // folder discovered beside the app / via MAGCS_DEM_DIR. The drape button
    // and the altitude profile both land here, so the DEM is handled
    // automatically whenever the operator already has one.
    const sources: SurfaceSource[] = [
      async () => {
        const remembered = get().folder ?? sessionStorage.getItem('maggcs.terrainFolder')
        return remembered ? await load(remembered) : null
      },
      // "Prepare survey area" (U9b) already fetched exactly the DTM the drape
      // needs; its session folder is a valid single-tile DEM, so reuse it and
      // spare the operator a manual folder pick after a prepare.
      async () => {
        const aoi = await surveyAoi()
        if (!aoi) return null
        const status = await invoke<SurveyPrepStatus>('survey_prepare_status', {
          lat: aoi.lat,
          lon: aoi.lon,
          widthM: aoi.width,
          heightM: aoi.height,
          outDir: null,
        })
        return status.state === 'ready' && status.folder ? await load(status.folder) : null
      },
      // A DEM shipped next to the app, or pointed at with MAGCS_DEM_DIR.
      async () => {
        const source = await invoke<string | null>('terrain3d_dem_source')
        return source ? await load(source) : null
      },
    ]
    // Nothing automatic panned out: ask for the dem-prep output folder — but
    // only when the caller allows a dialog (a background drape does not).
    if (allowDialog) {
      sources.push(async () => {
        const { open } = await import('@tauri-apps/plugin-dialog')
        const picked = await open({ multiple: false, directory: true })
        if (!picked || typeof picked !== 'string') return null
        sessionStorage.setItem('maggcs.terrainFolder', picked)
        return await load(picked)
      })
    }
    const resolved = await firstSurface(...sources)
    if (!resolved) set({ loading: false })
    return resolved
  },
  clear: () =>
    set((st) => ({ overlay: null, tile: null, tiles: [], folder: null, uncovered: false, loading: false, error: null, homeGroundAmsl: null, amslAnchorOffsetM: st.amslAnchorOffsetM })),
  start3d: async () => {
    // A build is already running (double-click): ignore.
    if (get().building3d) return
    set({ error3d: null })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      // Preferred path (no dialog): read the local DEM GeoTIFF folder and
      // serve a tileset built from it once, then reuse the cached tileset.
      const source = await invoke<string | null>('terrain3d_dem_source')
      if (source) {
        startBuildWatcher()
        set({ building3d: { stage: 'process', line: '' }, buildError3d: null })
        const tileset = await invoke<string>('terrain3d_build_from_dem', { maxzoom: null })
        await serveBuilt3d(tileset, source)
        set({ building3d: null })
        return
      }
      // No local DEM: build the mesh from the survey area's HRDEM session
      // (fetched once, then cached) — the same DTM the 2D drape/AGL uses.
      const aoi = await surveyAoi()
      if (aoi) {
        startBuildWatcher()
        set({ building3d: { stage: 'hrdem', line: '' }, buildError3d: null })
        const tileset = await invoke<string>('terrain3d_build_from_survey', {
          lat: aoi.lat,
          lon: aoi.lon,
          widthM: aoi.width,
          heightM: aoi.height,
          outDir: null,
          maxzoom: null,
        })
        await serveBuilt3d(tileset, tileset)
        set({ building3d: null })
        return
      }
      // No survey area either: fall back to the manual folder picker.
      const { open } = await import('@tauri-apps/plugin-dialog')
      const folder = await open({ multiple: false, directory: true })
      if (!folder || typeof folder !== 'string') return // cancelled
      const url = await invoke<string>('terrain3d_start', { dir: folder })
      sessionStorage.setItem('maggcs.terrain3dFolder', folder)
      set({ terrain3d: { url, folder }, error3d: null })
    } catch (e) {
      set({ building3d: null, error3d: String(e) })
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
  prepareSurvey: async (lat, lon, width, height) => {
    startBuildWatcher()
    set({ surveyPrep: { stage: 'hrdem', line: '' } })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const status = await invoke<SurveyPrepStatus>('survey_prepare', {
        lat,
        lon,
        widthM: width,
        heightM: height,
        outDir: null,
      })
      set({ surveyPrep: null })
      return status
    } catch (e) {
      set({ surveyPrep: null })
      // The backend signals a kill as the literal "cancelled": not an error.
      if (String(e).includes('cancelled')) return null
      throw e
    }
  },
  cancelSurvey: async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('survey_prepare_cancel')
    } catch {
      /* nothing was running */
    }
  },
}))

/** The survey-area AOI for the 3D mesh: the drawn polygon's bounding box when
 * a boundary is closed, else the pattern centre (else the map centre) with the
 * shared survey size. `null` when neither is set. The DEM must cover the whole
 * boundary — a 100 × 100 m default rectangle under a 6 ha polygon is exactly
 * the "waypoints outside the mesh" failure. */
async function surveyAoi(): Promise<{
  lat: number
  lon: number
  width: number
  height: number
} | null> {
  const { useMissionStore } = await import('./mission')
  const { useUiStore } = await import('./ui')
  const { usePolygonStore } = await import('./polygon')
  const { effectiveSurveyAoi } = await import('../util/surveyAoi')
  const polygon = usePolygonStore.getState()
  const size = useUiStore.getState().surveySize
  const center = useMissionStore.getState().patternCenter
  const aoi = effectiveSurveyAoi(polygon, center, size)
  if (aoi) return aoi
  const mapCenter = useUiStore.getState().mapCenter
  if (mapCenter) return { lat: mapCenter.lat, lon: mapCenter.lon, ...size }
  return null
}

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
 * the toggle on (the same state `start3d` would produce, minus the dialog).
 * `displayFolder` is what the popover names — the DEM source folder for the
 * auto path, the tileset itself otherwise. */
async function serveBuilt3d(tileset: string, displayFolder: string = tileset): Promise<void> {
  const { invoke } = await import('@tauri-apps/api/core')
  const url = await invoke<string>('terrain3d_start', { dir: tileset })
  sessionStorage.setItem('maggcs.terrain3dFolder', displayFolder)
  useTerrainStore.setState({
    terrain3d: { url, folder: displayFolder },
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
        // The survey-area fetch shares the event with the 3D pipeline; route
        // by stage so both log streams stay independent.
        if (e.payload.stage === 'hrdem') {
          useTerrainStore.setState({ surveyPrep: e.payload })
        } else {
          useTerrainStore.setState({ building3d: e.payload })
        }
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

// Dev-only handle for inspecting the app from the DevTools protocol / E2E
// scripts (same pattern as `window.__mgViewer` in `useCesiumViewer`).
if (import.meta.env?.DEV && typeof window !== 'undefined') {
  ;(window as unknown as { __mgTerrain?: unknown }).__mgTerrain = useTerrainStore
}
