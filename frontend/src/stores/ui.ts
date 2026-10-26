import { create } from 'zustand'
import { writeShared } from './sharedState'
import { applyFieldMode, applyThemePref, readFieldMode, readThemePref, saveFieldMode, saveThemePref, type ThemePref } from '../design-system/theme'

/** The five job stages (U1): free switching, no forced linearity. `plan`
 * keeps the four planning step cards; the map editing tools are `plan`-only
 * (see [`isPlanStage`]). */
export type View = 'prepare' | 'plan' | 'preflight' | 'fly' | 'review'
export type Theme = ThemePref
export type DockTab = 'profile' | 'qc' | 'log'
/** One planning step expanded at a time (P2 §4.2). */
export type PlanStep = 'area' | 'pattern' | 'waypoints' | 'sync'
/** Active map tool. `add` places a new waypoint on click; `polygon` draws a
 * survey boundary (both planning view only). */
export type MapTool = 'select' | 'add' | 'polygon' | 'center'
/** Basemap imagery on top of the offline grid. */
export type ImageryStyle = 'google-satellite' | 'google-hybrid'

export interface ViewConfig {
  rightTab: string
  dockTab: DockTab
}

export const VIEW_CONFIG: Record<View, ViewConfig> = {
  prepare: { rightTab: 'panels.prepare', dockTab: 'profile' },
  plan: { rightTab: 'panels.plan', dockTab: 'profile' },
  preflight: { rightTab: 'panels.preflight', dockTab: 'profile' },
  fly: { rightTab: 'panels.telemetry', dockTab: 'qc' },
  review: { rightTab: 'panels.review', dockTab: 'profile' },
}

/** The stages where the map editing tools (waypoint, polygon, centre) apply. */
export const isPlanStage = (view: View): boolean => view === 'plan'

interface UiState {
  view: View
  theme: Theme
  /** U12: the touch/outdoor profile. */
  fieldMode: boolean
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
  /** Bump to ask the map to frame the whole mission (post-Generate). */
  fitRequest: number
  requestFit: () => void
  /** Active map tool; `Esc` and leaving the planning view reset it to `select`. */
  mapTool: MapTool
  /** Draw a height stick and an AGL chip on every map waypoint (default on:
   * clearance is the number a survey plan is checked against). */
  showHeights: boolean
  /** Imagery style on top of the offline grid (always shown; no
   * grid-only mode — operator request 2026-10-10). */
  imageryStyle: ImageryStyle
  /** Offline graticule base layer. */
  showGrid: boolean
  /** U9b: outline the prepared DEM footprint on the map. */
  showCoverage: boolean
  /** U16: transparency of the hillshade overlay (clamped 0.2–1; the offline
   * grid stays readable underneath). */
  hillshadeOpacity: number
  setHillshadeOpacity: (v: number) => void
  /** U16: restore every layer preference to its default — imagery style,
   * toggles and opacity. Data sources and the mission are untouched. */
  resetLayers: () => void
  /** U17b: the plan field to reveal after clicking a blocker chip. `nonce`
   * forces the reveal effect to re-run when the same field is clicked twice. */
  planFocus: { field: string; nonce: number } | null
  /** U5: a preflight repair entry asks for the settings dialog (the dialog
   * state lives in TopBar; this request crosses the tree). */
  settingsRequest: 'connection' | 'vehicle' | 'appearance' | 'devices' | 'about' | null
  openSettings: (tab: 'connection' | 'vehicle' | 'appearance' | 'devices' | 'about') => void
  clearSettingsRequest: () => void
  /** Survey-area rectangle size (metres, U8). Shared by the Area step and the
   * 3D-mesh / prepare actions so they target the same AOI. */
  surveySize: { width: number; height: number }
  /** Survey-area azimuth (degrees, U8). */
  surveyHeading: number
  setView: (v: View) => void
  setTheme: (t: Theme) => void
  /** U12 field-mode toggle (Settings ▸ Appearance). */
  setFieldMode: (on: boolean) => void
  setPlanStep: (step: PlanStep) => void
  /** Open `step` and reveal `field` (scroll + highlight): the blocker-chip
   * navigation (U17b). */
  focusPlanField: (step: PlanStep, field: string) => void
  clearPlanFocus: () => void
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
  setImageryStyle: (style: ImageryStyle) => void
  toggleGrid: () => void
  toggleCoverage: () => void
  setSurveySize: (size: { width: number; height: number }) => void
  setSurveyHeading: (heading: number) => void
}

const DOCK_MIN = 96
const DOCK_MAX = 420
const DOCK_DEFAULT = 180

function narrowScreen(): boolean {
  return typeof window !== 'undefined' && window.innerWidth <= 1366
}

const initialTheme: Theme = typeof localStorage !== 'undefined' ? readThemePref() : 'dark'

// ---------------------------------------------------------------------------
// U16 layer preferences: persisted in `localStorage` so a restart keeps the
// operator's view. Deliberately a *view* store — data sources and the mission
// are never touched by these (the safety-critical vehicle marker and alerts
// are not toggleable at all).
// ---------------------------------------------------------------------------

const LAYERS_KEY = 'maggcs.layers'
const OPACITY_MIN = 0.2
const OPACITY_MAX = 1

interface LayerPrefs {
  showHeights: boolean
  imageryStyle: ImageryStyle
  showGrid: boolean
  showCoverage: boolean
  hillshadeOpacity: number
}

const DEFAULT_LAYERS: LayerPrefs = {
  showHeights: true,
  imageryStyle: 'google-satellite',
  showGrid: true,
  showCoverage: true,
  hillshadeOpacity: 0.65,
}

/** Clamp the overlay opacity into the readable range (U16 acceptance: the
 * range is limited — a fully transparent hillshade is indistinguishable from
 * "broken", a fully opaque one hides the base map). */
export function clampOpacity(v: number): number {
  if (!Number.isFinite(v)) return DEFAULT_LAYERS.hillshadeOpacity
  return Math.min(OPACITY_MAX, Math.max(OPACITY_MIN, v))
}

function readLayerPrefs(): LayerPrefs {
  try {
    const raw = localStorage.getItem(LAYERS_KEY)
    if (!raw) return { ...DEFAULT_LAYERS }
    const parsed = JSON.parse(raw) as Partial<LayerPrefs>
    return {
      ...DEFAULT_LAYERS,
      ...parsed,
      hillshadeOpacity: clampOpacity(parsed.hillshadeOpacity ?? DEFAULT_LAYERS.hillshadeOpacity),
    }
  } catch {
    return { ...DEFAULT_LAYERS }
  }
}

function saveLayerPrefs(p: LayerPrefs): void {
  try {
    localStorage.setItem(LAYERS_KEY, JSON.stringify(p))
  } catch {
    /* private mode */
  }
}

/** The current layer slice of the store (for persistence). */
function layerSnapshot(s: UiState): LayerPrefs {
  return {
    showHeights: s.showHeights,
    imageryStyle: s.imageryStyle,
    showGrid: s.showGrid,
    showCoverage: s.showCoverage,
    hillshadeOpacity: s.hillshadeOpacity,
  }
}

export const useUiStore = create<UiState>((set, get) => ({
  view: 'fly',
  theme: initialTheme,
  /** U12 field mode: touch ≥44 px, 14 px text, reduced motion. */
  fieldMode: typeof localStorage !== 'undefined' ? readFieldMode() : false,
  planStep: 'area',
  dockTab: 'qc',
  rightOpen: !narrowScreen(),
  dockOpen: true,
  dockHeight: narrowScreen() ? 160 : DOCK_DEFAULT,
  follow: true,
  map3d: true,
  hudCollapsed: false,
  mapCenter: null,
  fitRequest: 0,
  mapTool: 'select',
  // U16: the layer preferences persist in `localStorage` (a restarted app
  // keeps the operator's view); the two migrated toggles also ride the
  // shared state (A11) so a detached map agrees with the main window.
  ...readLayerPrefs(),
  hillshadeOpacity: readLayerPrefs().hillshadeOpacity,
  planFocus: null,
  settingsRequest: null,
  openSettings: (tab) => set({ settingsRequest: tab }),
  clearSettingsRequest: () => set({ settingsRequest: null }),
  surveySize: { width: 100, height: 100 },
  surveyHeading: 0,
  setView: (view) =>
    set(() => ({
      view,
      rightOpen: !narrowScreen(),
      // Editing tools belong to the plan stage (finding 18).
      ...(view === 'plan' ? {} : { mapTool: 'select' as const }),
    })),
  setTheme: (theme) => {
    saveThemePref(theme)
    applyThemePref(theme)
    set({ theme })
  },
  setFieldMode: (on) => {
    saveFieldMode(on)
    applyFieldMode(on)
    set({ fieldMode: on })
  },
  setPlanStep: (planStep) => set({ planStep }),
  focusPlanField: (planStep, field) =>
    set((s) => ({ planStep, planFocus: { field, nonce: (s.planFocus?.nonce ?? 0) + 1 } })),
  clearPlanFocus: () => set({ planFocus: null }),
  toggleRight: () => set((s) => ({ rightOpen: !s.rightOpen })),
  setDockOpen: (dockOpen) => set({ dockOpen }),
  setDockHeight: (h) => set({ dockHeight: Math.max(DOCK_MIN, Math.min(DOCK_MAX, h)) }),
  setDockTab: (dockTab) => set({ dockTab }),
  toggleFollow: () => set((s) => ({ follow: !s.follow })),
  setFollow: (follow) => set({ follow }),
  toggleMap3d: () => set((s) => ({ map3d: !s.map3d })),
  toggleHud: () => set((s) => ({ hudCollapsed: !s.hudCollapsed })),
  setMapCenter: (mapCenter) => set({ mapCenter }),
  requestFit: () => set((s) => ({ fitRequest: s.fitRequest + 1 })),
  setMapTool: (mapTool) => set({ mapTool }),
  toggleHeights: () =>
    set((s) => {
      const next = { showHeights: !s.showHeights }
      saveLayerPrefs({ ...layerSnapshot(s), ...next })
      return next
    }),
  setImageryStyle: (imageryStyle) =>
    set((s) => {
      saveLayerPrefs({ ...layerSnapshot(s), imageryStyle })
      return { imageryStyle }
    }),
  // A11 phase 1: the layer toggles are shared state — the local field is the
  // projection, the write goes through the versioned core store so a second
  // webview (the detached map, U15) sees the same truth.
  toggleGrid: () => {
    set((s) => {
      const next = { showGrid: !s.showGrid }
      saveLayerPrefs({ ...layerSnapshot(s), ...next })
      return next
    })
    void writeShared('layers.grid', get().showGrid)
  },
  toggleCoverage: () => {
    set((s) => {
      const next = { showCoverage: !s.showCoverage }
      saveLayerPrefs({ ...layerSnapshot(s), ...next })
      return next
    })
    void writeShared('layers.coverage', get().showCoverage)
  },
  setHillshadeOpacity: (v) =>
    set((s) => {
      const hillshadeOpacity = clampOpacity(v)
      saveLayerPrefs({ ...layerSnapshot(s), hillshadeOpacity })
      return { hillshadeOpacity }
    }),
  resetLayers: () => {
    set(() => ({ ...DEFAULT_LAYERS }))
    saveLayerPrefs({ ...DEFAULT_LAYERS })
    void writeShared('layers.grid', DEFAULT_LAYERS.showGrid)
    void writeShared('layers.coverage', DEFAULT_LAYERS.showCoverage)
  },
  setSurveySize: (surveySize) => set({ surveySize }),
  setSurveyHeading: (surveyHeading) => set({ surveyHeading }),
}))

applyThemePref(initialTheme)


// Dev-only handle for the Playwright checks (same pattern as `__mgMission`).
if (import.meta.env?.DEV && typeof window !== 'undefined') {
  ;(window as unknown as { __mgUi?: unknown }).__mgUi = useUiStore
}
