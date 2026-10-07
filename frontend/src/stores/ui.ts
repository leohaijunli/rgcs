import { create } from 'zustand'

export type View = 'planning' | 'flight' | 'data'
export type Theme = 'dark' | 'light'
export type DrawerId = 'missions' | 'vehicles' | 'layers' | null
export type DockTab = 'profile' | 'qc' | 'log'

export interface ViewConfig {
  /** Drawer section opened by default when switching to this view. */
  drawer: DrawerId
  rightTab: string
  dockTab: DockTab
}

export const VIEW_CONFIG: Record<View, ViewConfig> = {
  planning: { drawer: 'missions', rightTab: 'panels.properties', dockTab: 'profile' },
  flight: { drawer: 'vehicles', rightTab: 'panels.telemetry', dockTab: 'qc' },
  data: { drawer: 'layers', rightTab: 'panels.layers', dockTab: 'qc' },
}

interface UiState {
  view: View
  theme: Theme
  drawer: DrawerId
  dockTab: DockTab
  rightOpen: boolean
  dockOpen: boolean
  dockHeight: number
  follow: boolean
  map3d: boolean
  /** Last known map-camera centre in degrees, used as the default new-waypoint spot. */
  mapCenter: { lat: number; lon: number } | null
  dashboardOpen: boolean
  qcOpen: boolean
  setView: (v: View) => void
  setTheme: (t: Theme) => void
  toggleDrawer: (id: NonNullable<DrawerId>) => void
  closeDrawer: () => void
  toggleRight: () => void
  setDockOpen: (open: boolean) => void
  setDockHeight: (h: number) => void
  setDockTab: (tab: DockTab) => void
  toggleFollow: () => void
  setFollow: (follow: boolean) => void
  toggleMap3d: () => void
  setMapCenter: (c: { lat: number; lon: number }) => void
  setDashboardOpen: (open: boolean) => void
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
  drawer: narrowScreen() ? null : 'vehicles',
  dockTab: 'qc',
  rightOpen: !narrowScreen(),
  dockOpen: true,
  dockHeight: narrowScreen() ? 160 : DOCK_DEFAULT,
  follow: true,
  map3d: true,
  mapCenter: null,
  dashboardOpen: false,
  qcOpen: false,
  setView: (view) =>
    set(() => ({
      view,
      drawer: VIEW_CONFIG[view].drawer,
      rightOpen: !narrowScreen(),
    })),
  setTheme: (theme) => {
    applyTheme(theme)
    set({ theme })
  },
  toggleDrawer: (id) => set((s) => ({ drawer: s.drawer === id ? null : id })),
  closeDrawer: () => set({ drawer: null }),
  toggleRight: () => set((s) => ({ rightOpen: !s.rightOpen })),
  setDockOpen: (dockOpen) => set({ dockOpen }),
  setDockHeight: (h) => set({ dockHeight: Math.max(DOCK_MIN, Math.min(DOCK_MAX, h)) }),
  setDockTab: (dockTab) => set({ dockTab }),
  toggleFollow: () => set((s) => ({ follow: !s.follow })),
  setFollow: (follow) => set({ follow }),
  toggleMap3d: () => set((s) => ({ map3d: !s.map3d })),
  setMapCenter: (mapCenter) => set({ mapCenter }),
  setDashboardOpen: (dashboardOpen) => set({ dashboardOpen }),
  setQcOpen: (qcOpen) => set({ qcOpen }),
}))

applyTheme(initialTheme)
