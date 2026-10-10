import { create } from 'zustand'

export type View = 'planning' | 'flight'
export type Theme = 'dark' | 'light'
export type DockTab = 'profile' | 'qc' | 'log'
/** One planning step expanded at a time (P2 §4.2). */
export type PlanStep = 'area' | 'pattern' | 'waypoints' | 'sync'
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
  planStep: PlanStep
  dockTab: DockTab
  rightOpen: boolean
  dockOpen: boolean
  dockHeight: number
  follow: boolean
  map3d: boolean
  /** Flight-instrument HUD collapsed to a pill (bottom-left, §4.5). */
  hudCollapsed: boolean
  /** Last known map-camera centre in degrees, used as the default new-waypoint spot. */
  mapCenter: { lat: number; lon: number } | null
  /** Active map tool; `Esc` and leaving the planning view reset it to `select`. */
  mapTool: MapTool
  /** Draw a height stick and an AGL chip on every map waypoint (default on:
   * clearance is the number a survey plan is checked against). */
  showHeights: boolean
  /** OSM imagery layer on top of the offline grid. */
  showImagery: boolean
  /** Offline graticule base layer. */
  showGrid: boolean
  setView: (v: View) => void
  setTheme: (t: Theme) => void
  setPlanStep: (step: PlanStep) => void
  toggleRight: () => void
  setDockOpen: (open: boolean) => void
  setDockHeight: (h: number) => void
  setDockTab: (tab: DockTab) => void
  toggleFollow: () => void
  setFollow: (follow: boolean) => void
  toggleMap3d: () => void
  toggleHud: () => void
  setMapCenter: (c: { lat: number; lon: number }) => void
  setMapTool: (tool: MapTool) => void
  toggleHeights: () => void
  toggleImagery: () => void
  toggleGrid: () => void
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
  planStep: 'area',
  dockTab: 'qc',
  rightOpen: !narrowScreen(),
  dockOpen: true,
  dockHeight: narrowScreen() ? 160 : DOCK_DEFAULT,
  follow: true,
  map3d: true,
  hudCollapsed: false,
  mapCenter: null,
  mapTool: 'select',
  showHeights: true,
  showImagery: true,
  showGrid: true,
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
  setPlanStep: (planStep) => set({ planStep }),
  toggleRight: () => set((s) => ({ rightOpen: !s.rightOpen })),
  setDockOpen: (dockOpen) => set({ dockOpen }),
  setDockHeight: (h) => set({ dockHeight: Math.max(DOCK_MIN, Math.min(DOCK_MAX, h)) }),
  setDockTab: (dockTab) => set({ dockTab }),
  toggleFollow: () => set((s) => ({ follow: !s.follow })),
  setFollow: (follow) => set({ follow }),
  toggleMap3d: () => set((s) => ({ map3d: !s.map3d })),
  toggleHud: () => set((s) => ({ hudCollapsed: !s.hudCollapsed })),
  setMapCenter: (mapCenter) => set({ mapCenter }),
  setMapTool: (mapTool) => set({ mapTool }),
  toggleHeights: () => set((s) => ({ showHeights: !s.showHeights })),
  toggleImagery: () => set((s) => ({ showImagery: !s.showImagery })),
  toggleGrid: () => set((s) => ({ showGrid: !s.showGrid })),
}))

applyTheme(initialTheme)
