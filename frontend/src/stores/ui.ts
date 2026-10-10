import { create } from 'zustand'

export type View = 'planning' | 'flight'
export type Theme = 'dark' | 'light'
export type DockTab = 'profile' | 'qc' | 'log'
/** Active map tool. `add` places a new waypoint on click; `polygon` draws a
 * survey boundary (both planning view only). */
export type MapTool = 'select' | 'add' | 'polygon'

export interface ViewConfig {
  rightTab: string
  dockTab: DockTab
}

export const VIEW_CONFIG: Record<View, ViewConfig> = {
  planning: { rightTab: 'panels.properties', dockTab: 'profile' },
  flight: { rightTab: 'panels.telemetry', dockTab: 'qc' },
}

interface UiState {
  view: View
  theme: Theme
  dockTab: DockTab
  rightOpen: boolean
  dockOpen: boolean
  dockHeight: number
  follow: boolean
  map3d: boolean
  /** Last known map-camera centre in degrees, used as the default new-waypoint spot. */
  mapCenter: { lat: number; lon: number } | null
  /** Active map tool; `Esc` and leaving the planning view reset it to `select`. */
  mapTool: MapTool
  /** Draw a height stick and an altitude label on every map waypoint. */
  showHeights: boolean
  /** OSM imagery layer on top of the offline grid. */
  showImagery: boolean
  /** Offline graticule base layer. */
  showGrid: boolean
  qcOpen: boolean
  setView: (v: View) => void
  setTheme: (t: Theme) => void
  toggleRight: () => void
  setDockOpen: (open: boolean) => void
  setDockHeight: (h: number) => void
  setDockTab: (tab: DockTab) => void
  toggleFollow: () => void
  setFollow: (follow: boolean) => void
  toggleMap3d: () => void
  setMapCenter: (c: { lat: number; lon: number }) => void
  setMapTool: (tool: MapTool) => void
  toggleHeights: () => void
  toggleImagery: () => void
  toggleGrid: () => void
  setQcOpen: (open: boolean) => void
}

const DOCK_MIN = 96
const DOCK_MAX = 420
const DOCK_DEFAULT = 180

function applyTheme(theme: Theme) {
  document.documentElement.dataset.theme = theme
  try {
    localStorage.setItem('maggcs.theme', theme)
  } catch {
    /* private mode */
  }
}

function narrowScreen(): boolean {
  return typeof window !== 'undefined' && window.innerWidth <= 1366
}

const initialTheme: Theme =
  (typeof localStorage !== 'undefined' &&
    (localStorage.getItem('maggcs.theme') as Theme | null)) ||
  'dark'

export const useUiStore = create<UiState>((set) => ({
  view: 'flight',
  theme: initialTheme,
  dockTab: 'qc',
  rightOpen: !narrowScreen(),
  dockOpen: true,
  dockHeight: narrowScreen() ? 160 : DOCK_DEFAULT,
  follow: true,
  map3d: true,
  mapCenter: null,
  mapTool: 'select',
  showHeights: false,
  showImagery: true,
  showGrid: true,
  qcOpen: false,
  setView: (view) =>
    set(() => ({
      view,
      rightOpen: !narrowScreen(),
      // Editing tools belong to the planning view (finding 18).
      ...(view === 'planning' ? {} : { mapTool: 'select' as const }),
    })),
  setTheme: (theme) => {
    applyTheme(theme)
    set({ theme })
  },
  toggleRight: () => set((s) => ({ rightOpen: !s.rightOpen })),
  setDockOpen: (dockOpen) => set({ dockOpen }),
  setDockHeight: (h) => set({ dockHeight: Math.max(DOCK_MIN, Math.min(DOCK_MAX, h)) }),
  setDockTab: (dockTab) => set({ dockTab }),
  toggleFollow: () => set((s) => ({ follow: !s.follow })),
  setFollow: (follow) => set({ follow }),
  toggleMap3d: () => set((s) => ({ map3d: !s.map3d })),
  setMapCenter: (mapCenter) => set({ mapCenter }),
  setMapTool: (mapTool) => set({ mapTool }),
  toggleHeights: () => set((s) => ({ showHeights: !s.showHeights })),
  toggleImagery: () => set((s) => ({ showImagery: !s.showImagery })),
  toggleGrid: () => set((s) => ({ showGrid: !s.showGrid })),
  setQcOpen: (qcOpen) => set({ qcOpen }),
}))

applyTheme(initialTheme)
