// Terrain overlay state (plan A): the dem-prep hillshade layer, loaded from
// a dem-prep output folder (N1). The folder may hold many tiles under a
// folder-level `index.json` — the store discovers them and auto-picks the
// tile covering the survey area; the drape reuses the same pick.

import { create } from 'zustand'
import type { DemBounds } from '../generated-types/DemBounds'

/** One tile from `terrain_discover` (absolute folder + placement). */
export interface DiscoveredTile {
  path: string
  folder: string
  bounds: DemBounds
  version: string
  has_dsm: boolean
}

export interface TerrainOverlay {
  /** Data URL of the hillshade PNG (CSP allows img-src data:). */
  url: string
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
  load: () => Promise<void>
  /** Pick + load a tile against the current survey area (no dialog). */
  repick: () => Promise<void>
  /** Ensure a drapeable tile path (auto-pick; asks for a folder only when
   * nothing was loaded yet). Returns null when the area is uncovered. */
  ensureTile: () => Promise<string | null>
  clear: () => void
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
}))

type Setter = (partial: Partial<TerrainState>) => void

/** Discover tiles under `folder`, auto-pick by survey area, load overlay. */
async function discoverAndLoad(
  invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>,
  folder: string,
  set: Setter,
): Promise<void> {
  const { tiles } = await invoke<{ tiles: DiscoveredTile[] }>('terrain_discover', {
    folder,
  })
  const bounds = await surveyBounds()
  const tile = pickTile(tiles, bounds)
  set({ folder, tiles, tile, uncovered: tiles.length > 0 && tile == null })
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
