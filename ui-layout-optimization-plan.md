# MagGCS Main-UI Layout Optimization Plan (companion to DEVELOPMENT_PLAN v2.2)

2026-10-09 (rev. 2026-10-10) · Scope: the main window layout (TopBar, left rail/drawer, map
overlays, right panel, bottom dock). Not in scope: the Signal Inspector window
(ADR-016), Settings dialog internals, Cesium rendering.

Based on a read of `App.tsx`, `stores/ui.ts`, `TopBar`, `IconRail`, `Drawer`,
`RightInspector`, `MapToolbar`, `MapView`, `Dock`, `Hud`, `FlightCommands`,
`PlanningPanel`, `PatternPanel`, `MissionProgressCard`, `PfdCluster`,
`TelemetryDialog`, `QcDialog`, `i18n/en.ts`. Nothing was compiled or run; the
committed `frontend/screenshots/` predate the current code and were not used as
evidence. Items marked *verify* need a live check.

Effort sizes are relative: S < M < L.

---

## 1. Decisions (2026-10-09)

| # | Decision |
| --- | --- |
| D1 | **Remove the Data view.** Data inspection is the job of the separate Signal Inspector window. Main navigation becomes **Plan / Fly**. |
| D2 | **Delete the instrument dashboard** (`TelemetryDialog`, `PfdCluster`, the toolbar button, `dashboardOpen` state). It is never mounted, so it shows nothing. The existing map HUD stays as the only flight-dynamics readout (§4.5). No pop-out window. |
| D3 | **Plan panel lives on the right**, as a step-by-step workflow (§4.2). The left rail and drawer are removed. |
| D4 | **One mission per task, single vehicle.** No mission list, no vehicle list. Nothing in this plan introduces multi-vehicle or multi-mission UI. (Issue #21's multi-vehicle-ready state is a backend concern and is untouched.) |

*Rev. 2026-10-10:* D2 changed from "keep the dashboard as a compact/pop-out PFD"
to "delete it". The `Pfd` component, the pop-out window and the former P5 are
removed from this plan.

---

## 2. Findings

### 2.1 Navigation

| # | Finding | Evidence |
| --- | --- | --- |
| N1 | Two-level navigation (view × drawer) of which only the diagonal is used. Switching view force-resets the drawer. | `VIEW_CONFIG` in `stores/ui.ts` |
| N2 | Left drawer content is thin or duplicated: Vehicles = 2 rows; Layers = 2 toggles + 2 "not loaded" stubs; Missions = a second copy of the right panel's list (Plan) or of `MissionProgressCard` (Fly). | `Drawer.tsx`, `RightInspector.tsx` |
| N3 | The left rail + drawer cost 48 + 280 px (≈ 24% of a 1366 px window) for that content. | `IconRail.tsx`, `Drawer.tsx` |
| N4 | The Data view is an empty placeholder ("arrives in Phase 4") yet is a top-level tab. | `views.dataPlaceholder` |

### 2.2 Plan workflow is scattered

| # | Finding |
| --- | --- |
| P1 | Controls for one task live in six places: map toolbar (add, polygon), right panel, left drawer, map overlay (polygon readout + clear), Dock `Profile / AGL` (placeholder), TopBar (none). |
| P2 | Right panel order does not match the workflow: altitude mode → pattern parameters → list → coordinate entry → add/remove → upload/download/clear → import/export/clear plan → status. Upload is at the bottom; the polygon (the first thing to do) is not in the panel at all. |
| P3 | Three "clear" actions with near-identical labels and different blast radius: `Clear` (FC mission), `Clear plan` (local), polygon `Clear`. |
| P4 | Sync status is rendered in three places (panel status line, drawer status block, busy/progress line). |
| P5 | `ItemEditor` is the last block of the scrolling panel, so selecting a waypoint often changes nothing visible. |
| P6 | `PatternPanel` shows ~20 numeric fields at once with no common/advanced split. |
| P7 | Altitude mode is a mission-wide setting presented as just another card. |

### 2.3 Clutter and dead controls

| # | Finding | Evidence |
| --- | --- | --- |
| C1 | Map toolbar: 10 buttons (Fly) / 12 (Plan) at 44 px ≈ 530 px+ tall. At 1366×768 the map area is ≈ 560 px with the dock open, so it barely fits. | `MapToolbar.tsx` |
| C2 | **Dashboard button is dead**: `TelemetryDialog` is never mounted (`App.tsx` mounts only `QcDialog`). | grep: no `<TelemetryDialog` |
| C3 | **Measure button is a no-op**: `onToggleMeasure={() => undefined}`. | `MapView.tsx` |
| C4 | Toolbar mixes three kinds of controls: edit tools, view controls, window launchers. | `MapToolbar.tsx` |
| C5 | TopBar: logo + 3 tabs + 6 pills + 2 icon buttons. Only the `FC` pill is clickable but looks like the rest. `Devices` is read-only. | `TopBar.tsx` |
| C6 | Same quantities shown 3 times (speed, altitude, battery, GPS): TopBar pills, HUD, right-panel Telemetry card. | `TopBar`, `Hud`, `FlightInspector` |
| C7 | `FlightCommands` and `MapToolbar` are both `absolute right-3 top-3`, and `FlightCommands` has `z-10`, so in the Fly view the command bar can cover the top toolbar buttons (*verify*). | `FlightCommands.tsx`, `MapToolbar.tsx` |
| C8 | Stale copy: `panels.commandsNote` says commands "arrive with Phase 5" although `FlightCommands` exists. | `i18n/en.ts` |
| C9 | Placeholders visible to users: `views.dataPlaceholder`, `views.profilePlaceholder`, `dock.logPlaceholder`, Layers "not loaded" items, Settings → Logs placeholder. | `i18n/en.ts`, `Drawer.tsx` |
| C10 | Dock takes 160–180 px by default and holds only placeholders, while the one real chart (`QcDialog`) is in a modal. `DockTab` includes `qc` but `TABS` does not. | `Dock.tsx`, `QcDialog.tsx` |
| C11 | HUD sits bottom-left over the Cesium credit logo (*verify* in the running app). | `Hud.tsx` |

---

## 3. Design rules

1. **One region, one job.** TopBar = navigation + vehicle health. Map = spatial
   tools + flight instruments. Right panel = the task for the current view.
   Dock = time-series / profile.
2. **A quantity is shown once.** Health (armed, mode, link, GPS fix, battery)
   lives in the TopBar only. Flight dynamics (attitude, speed, altitude,
   heading) live in the map HUD only. The right panel does not repeat either.
3. **Task order = layout order.** Plan: area → pattern → waypoints → sync.
   Fly: commands → progress → alerts.
4. **No fake affordances.** A control without a working handler, a tab without
   content and a "not loaded" row are removed, not disabled.
5. **≤ 6 persistent map buttons**; the rest go into two grouped popovers.
6. **Overlays never overlap.** Each map corner has one owner (§4.3).

---

## 4. Target design

### 4.1 Frame

```
┌──────────────────────────────────────────────────────────────────────┐
│ MagGCS [Plan|Fly]        ARMED MODE GPS BATT LINK▾    ⌁ Inspector  ⚙ │ TopBar 48
├────────────────────────────────────────────────────┬─────────────────┤
│ ┌tools┐                                            │  Right panel    │
│ │ ... │                                            │  (per view,     │
│ └─────┘                                            │   264 px,       │
│ ┌─── HUD ─────┐                      cursor/pos    │   collapsible)  │
│ └─────────────┘  Cesium credit                     │                 │
├────────────────────────────────────────────────────┴─────────────────┤
│ Dock: tabs only when they have content; collapsed by default (28 px) │
└──────────────────────────────────────────────────────────────────────┘
```

Removed: `IconRail`, `Drawer`, Data view, the instrument dashboard
(`TelemetryDialog`, `PfdCluster`), Dock placeholders.

### 4.2 Right panel, Plan view

Header (always visible): **altitude mode** segmented control + one **sync badge**
(single source for sync state, replaces P4).

Four collapsible step cards, one expanded at a time:

1. **Area**: Draw polygon / rectangle / manual centre; area and perimeter
   readout and self-intersection error (moved from the map overlay); `Clear area`.
2. **Pattern**: Sweep / Cloverleaf. *Common* fields (spacing, line azimuth,
   altitude, speed) always shown; *Advanced* (tie spacing/azimuth, lead-in/out,
   alternate, petals, samples, …) folded. `Generate` / `Regenerate`.
3. **Waypoints**: list (with distance-from-vehicle), coordinate entry, `Add`.
   Selecting an item expands `ItemEditor` **inline under that row**.
4. **Sync**: primary `Upload to FC`; `Download` secondary; a `⋯` menu holds
   `Import .plan`, `Export .plan`, `Clear local plan` (two-click confirm) and
   `Clear FC mission` (danger style, two-click confirm). Progress and errors
   render directly under the primary button.

Only one "clear" per scope exists and each is named for its scope.

### 4.3 Map overlays: corner ownership

| Corner | Owner | Notes |
| --- | --- | --- |
| top-left | tool strip (vertical) | Moves from top-right so the right-side panel/commands never collide with it. |
| top-centre | transient hints and `MOCK DATA` banner | One slot; hints queue behind the banner. |
| top-right | nothing (right panel edge) | |
| bottom-left | HUD | Sits above the Cesium credit. |
| bottom-right | cursor/vehicle coordinate readout | Moved out of the toolbar. |

Tool strip contents:

| View | Visible buttons |
| --- | --- |
| Plan | Select · Add waypoint · Draw polygon · Layers ▸ · View ▸ |
| Fly | Follow · 2D/3D · Layers ▸ · View ▸ |

- **Layers ▸** popover: Imagery, Grid, Waypoint heights (and later DTM/DSM,
  mag layers as Phase 3/4 land).
- **View ▸** popover: North up, Go home, Zoom to mission, 2D/3D (Plan), and
  Measure once it has a handler.
- Tool tooltips show shortcuts (`W`, `P`, `Esc`); this stays compatible with
  `improve_plan.md` WS-G (tool state machine; `insert`/`home` tools arrive via
  segment handles and the context menu, not new buttons).

### 4.4 Right panel, Fly view and TopBar

Fly panel, top to bottom: **Commands** (Pause / Continue / RTL, existing
two-click confirm, existing ack status line) → **Mission progress card** →
**Active-line list** (current `FlightMissionSection`) → **Alerts** (QC, link,
GPS stale). No telemetry card.

TopBar: `Plan | Fly`, then status pills `ARMED · MODE · GPS(fix + sv) · BATT ·
LINK`, then `Signal inspector` and `Settings` icon buttons.

- `LINK ▾` is the only interactive pill and shows a chevron. Its popover
  carries what the Vehicles drawer showed (system id, vehicle type, autopilot),
  last heartbeat age, endpoint, Connect/Disconnect and "Open connection settings".
- `Devices` pill is removed (already available as Settings → Devices).
- `Signal inspector` gets a line-chart icon and a visible label on wide screens
  (it currently looks like an activity/notification icon).

### 4.5 Instrument HUD (D2)

The dashboard (`TelemetryDialog` + `PfdCluster` + its toolbar button + the
`dashboardOpen` state) is deleted. The existing map `Hud` stays as the single
flight-dynamics readout:

- Content: attitude indicator, speed, heading, relative altitude, AMSL.
  Battery and satellite count are removed from it (they are TopBar health,
  design rule 2).
- Collapsible (`hudCollapsed`), bottom-left above the Cesium credit; the
  `STALE` overlay is unchanged.
- No pop-out window and no second telemetry feed. A larger instrument view, if
  wanted later, is a new proposal and not part of this plan.
- `VerticalTape` is only imported by `PfdCluster` (checked by grep), so it is
  deleted too. `AttitudeIndicator` is used by `Hud` and stays.

### 4.6 Dock

- Collapsed by default (28 px tab strip); resizable when open (existing drag handle).
- A tab exists only when it has content:
  - **QC** (Fly): `QcDialog`'s uPlot moves here; becomes real QC curves in Phase 4.
  - **Profile / AGL** (Plan): appears with Phase 3b (DEM).
  - **Log**: appears when there is a mission/command event log (Phase 5).
- With no tab available the dock is not rendered at all.

### 4.7 Narrow screens (≤ 1366 px)

Right panel closed by default with an edge toggle; tool strip and HUD
fit the ≈ 560 px map height by construction (5 + 1 buttons ≈ 270 px; HUD
≈ 190 px; both are left-anchored but must not overlap, see acceptance A2).
Touch targets stay ≥ 44 px (UI spec §6); we reduce the *count*, not the size.

---

## 5. State and component contracts

Signatures first (AGENTS rule 2); implement after confirmation.

```ts
// stores/ui.ts
export type View = 'planning' | 'flight'            // 'data' removed
export type DockTab = 'qc' | 'profile' | 'log'
export type PlanStep = 'area' | 'pattern' | 'waypoints' | 'sync'

interface UiState {
  view: View; theme: Theme
  rightOpen: boolean
  planStep: PlanStep
  dockOpen: boolean; dockHeight: number; dockTab: DockTab | null
  hudCollapsed: boolean
  follow: boolean; map3d: boolean
  mapTool: MapTool                                   // 'select' | 'add' | 'polygon'
  showHeights: boolean; showImagery: boolean; showGrid: boolean
  mapCenter: { lat: number; lon: number } | null
  // removed: drawer, closeDrawer, toggleDrawer, dashboardOpen, setDashboardOpen,
  //          qcOpen, setQcOpen, VIEW_CONFIG.drawer
}

// components/panels/plan/PlanSteps.tsx
export default function PlanSteps(): JSX.Element
// components/panels/plan/{AreaStep,PatternStep,WaypointsStep,SyncStep}.tsx
// components/MapToolbar.tsx
export default function MapToolbar(props: {
  onGoHome: () => void; onNorth: () => void; onZoomToMission: () => void
}): JSX.Element
// components/LinkPopover.tsx
export default function LinkPopover(): JSX.Element  // wraps the FC pill
```

Fate of existing components:

| Component | Fate |
| --- | --- |
| `IconRail`, `Drawer` | delete; content redistributed (§4) |
| `RightInspector` | keeps shell; Plan → `PlanSteps`, Fly → commands + progress + list + alerts |
| `PlanningPanel`, `PatternPanel` | split into step components |
| `FlightCommands` | moves into the Fly panel (no longer absolutely positioned on the map) |
| `Hud` | keep; drop battery/SV readouts, add collapse |
| `PfdCluster`, `TelemetryDialog`, `VerticalTape` | delete (D2) |
| `QcDialog` | content moves to Dock `QC` tab |
| `MapToolbar` | rewritten per §4.3 |
| `MockBanner`, `ErrorBanner` | keep; re-check slot/position against §4.3 |

i18n keys to remove: `nav.data`, `panels.missions/vehicles/layers/tasks`,
`panels.commandsNote`, `panels.telemetry` (card), `views.*Placeholder`,
`dock.logPlaceholder`, `dock.qcPlaceholder`, `map.dashboard`; `map.measure`
only while it has no handler. New keys go through `check:i18n`.

---

## 6. Work packages

Each package is small, independently shippable, keeps `tsc --noEmit` green and
ends with updated screenshots (AGENTS rule 4).

| WP | Content | Effort | Depends on |
| --- | --- | --- | --- |
| **P0** Cleanup | Fix C7 (no overlap); delete the dashboard (button, `TelemetryDialog`, `PfdCluster`, `VerticalTape`, `dashboardOpen`) and the dead measure button; remove stale/placeholder strings (C8, C9); hide placeholder dock tabs. | S | none |
| **P1** Remove Data view and left rail | Delete `IconRail`/`Drawer`; `View` → `planning \| flight`; move Vehicles into `LinkPopover`; move layer toggles into a `Layers ▸` popover (temporary entry in the old toolbar); move waypoint list into the right panel. | M | P0 |
| **P2** Plan panel rework | `PlanSteps` + step components; inline `ItemEditor`; single sync badge; `⋯` menu; one clear per scope; polygon readout moved into Area step. | L | P1 |
| **P3** Tool strip and TopBar | New `MapToolbar` (§4.3) at top-left; `Layers ▸`/`View ▸` popovers; TopBar pills trimmed; `Signal inspector` icon; coordinate readout bottom-right. | M | P1 |
| **P4** HUD and telemetry dedup | Trim battery/SV from `Hud`, add collapse; delete the right-panel Telemetry card; `FlightCommands` into the Fly panel. | S | P3 |
| **P5** Dock | Collapsed default; `QC` tab hosts the existing plot; tabs only with content. | S | P1 |
| **P6** Regression | Update `scripts/screenshot.mjs` views (Plan/Fly only) and baselines; run all `check:*` scripts. | S | all |

Suggested order: P0 → P1 → P2 ∥ P3 → P4 → P5 → P6. P0 + P1 are low-risk
and remove most of the visual clutter on their own.

---

## 7. Acceptance (automatable where possible)

| # | Check |
| --- | --- |
| A1 | At 1920×1080 and 1366×768, in both views, every overlay bounding box lies inside the map container (Playwright `boundingBox`). |
| A2 | No two overlays (tool strip, HUD, banners, hints, coordinate readout) have intersecting bounding boxes at either resolution, in either view, with the right panel open and closed. |
| A3 | Click-through smoke test: each persistent toolbar button, popover item and TopBar pill that is shown changes store state or DOM when clicked (no dead controls). |
| A4 | Persistent map buttons ≤ 6 per view; TopBar has ≤ 5 status pills. |
| A5 | In the Fly view, speed, altitude and heading each appear in exactly one element (`data-testid` count = 1); battery % and GPS fix appear only in the TopBar. |
| A6 | Plan flow on mock data (draw polygon → generate → upload): at 1920×1080 no scrolling of the right panel is needed to reach each step's primary action; at 1366×768 at most one scroll. |
| A7 | Exactly one "clear" control per scope is visible (area / local plan / FC mission); FC clear requires the second click. |
| A8 | Selecting a waypoint shows its editor within the visible part of the panel (inline under the row). |
| A9 | `check:i18n` additionally fails on user-facing strings matching `Phase \d` or `arrives`. |
| A10 | `check:colors` and `check:contrast` pass for both themes; map stays ≥ 30 fps with panels open (existing UI acceptance, DEVELOPMENT_PLAN §6). |
| A11 | HUD: mock telemetry drives attitude, speed, heading and altitude; the `STALE` overlay appears > 3 s after the last sample. |

---

## 8. Documentation changes

- `docs/DEVELOPMENT_PLAN.md` §6 *Layout*: top = `Plan / Fly` + status bar;
  remove the Left bullet (mission/vehicle/layer drawers); Right = task panel
  (Plan steps / Fly commands+progress); Bottom = dock with content-gated tabs.
  Update the UI acceptance line to reference A1–A11.
- `docs/DEVELOPMENT_PLAN.md` Phase 4: the wording "flight-view HUD and bottom QC
  curve" stays valid; the QC curve lives in the Dock `QC` tab.
- `issues.md`: open one issue per WP (P0–P6) under P3 · frontend; #35 ("Layers
  panel is decorative") is resolved by P1/P3, #37 by the Fly panel (already
  partly done).
- `docs/README.md`: list this file.

---

## 9. Open questions

1. Keep `Follow` on the Plan view's tool strip? (This plan: Fly only; Plan has
   no live vehicle focus requirement.)
2. Measure tool: implement under `View ▸`, or drop until WS-G (`improve_plan.md`)
   builds the tool state machine? (This plan: hidden until implemented.)
3. Show the HUD in the Plan view too? (This plan: yes, collapsible; the vehicle
   may already be connected while planning.)
