// Terrain overlay state (plan A): the dem-prep hillshade layer, loaded from
// a tile folder (dtm/dsm/hillshade.png + dem.json) and draped over the
// imagery by its geographic bounds.

import { create } from 'zustand'

export interface TerrainOverlay {
  /** Data URL of the hillshade PNG (CSP allows img-src data:). */
  url: string
  bounds: { north: number; south: number; east: number; west: number }
  /** dem.json version (sha256[..16]) — mission archives reference it. */
  version: string
  /** True while a load is in flight. */
  loading: boolean
}

interface TerrainState {
  overlay: TerrainOverlay | null
  /** Error from the last load attempt (shown in the Layers popover). */
  error: string | null
  load: () => Promise<void>
  clear: () => void
}

export const useTerrainStore = create<TerrainState>((set) => ({
  overlay: null,
  error: null,
  load: async () => {
    set({ error: null })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const { open } = await import('@tauri-apps/plugin-dialog')
      const folder = await open({ multiple: false, directory: true })
      if (!folder || typeof folder !== 'string') return
      const overlay = await invoke<Omit<TerrainOverlay, 'loading'>>(
        'terrain_load',
        { folder },
      )
      set({ overlay: { ...overlay, loading: false }, error: null })
    } catch (e) {
      set({ error: String(e) })
    }
  },
  clear: () => set({ overlay: null, error: null }),
}))
