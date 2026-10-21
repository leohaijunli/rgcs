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
  /** Colour the flown track by height above ground (legend on the map). */
  trailByAgl: boolean
  /** Imagery style on top of the offline grid (always shown; no
   * grid-only mode — operator request 2026-10-10). */
  imageryStyle: ImageryStyle
  /** Offline graticule base layer. */
  showGrid: boolean
  /** U9b: outline the prepared DEM footprint on the map. */
  showCoverage: boolean
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
  toggleTrailByAgl: () => void
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
  showHeights: true,
  trailByAgl: true,
  imageryStyle: 'google-satellite',
  showGrid: true,
  showCoverage: true,
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
  toggleHeights: () => set((s) => ({ showHeights: !s.showHeights })),
  toggleTrailByAgl: () => set((s) => ({ trailByAgl: !s.trailByAgl })),
  setImageryStyle: (imageryStyle) => set({ imageryStyle }),
  // A11 phase 1: the layer toggles are shared state — the local field is the
  // projection, the write goes through the versioned core store so a second
  // webview (the detached map, U15) sees the same truth.
  toggleGrid: () => {
    set((s) => ({ showGrid: !s.showGrid }))
    void writeShared('layers.grid', get().showGrid)
  },
  toggleCoverage: () => {
    set((s) => ({ showCoverage: !s.showCoverage }))
    void writeShared('layers.coverage', get().showCoverage)
  },
  setSurveySize: (surveySize) => set({ surveySize }),
  setSurveyHeading: (surveyHeading) => set({ surveyHeading }),
}))

applyThemePref(initialTheme)
