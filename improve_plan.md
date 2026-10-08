# MagGCS Flight Plan Improvement Plan (companion to DEVELOPMENT_PLAN v2.2)

2026-10-07 · Scope: Phase 1 (mission protocol + planning view), Phase 3
(terrain + survey planner), the §5 height conventions they depend on, and the
map point-picking UX. Based on a read of `docs/DEVELOPMENT_PLAN.md`,
`issues.md`, `HANDOFF.md`, `core::mission`, `core::height`,
`frontend/src/stores/mission.ts`, `PlanningPanel.tsx`, `mission/planfile.ts`,
`cesium/waypoints.ts`, `hooks/useCesiumViewer.ts`, `stores/ui.ts`,
`MapToolbar.tsx`, ADR-004/005. Nothing was compiled or run; items marked
*verify* need a live check.

Effort sizes are relative: S < M < L.

---

## 1. Evaluation

**Strong**

- The Phase 1 protocol work is solid: retransmit ticks, foreign-source
  filtering, a lossy fake-FC harness, read-back verification.
- ADR-005 (GCS-side draping) and ADR-006 (AMSL working datum) are the right
  calls for a deterministic, auditable survey.

**Weak or at risk**

| # | Finding | Why it matters |
| --- | --- | --- |
| 1 | `setAltitudeMode` only sets the frame of *new* waypoints. | Switching mode leaves existing items on the old frame; the same `z` silently changes meaning. |
| 2 | `agl` mode maps to `GLOBAL_TERRAIN_ALT_INT` (onboard terrain). | Contradicts "path A first" (ADR-005). Whether PX4 v1.17 honours this frame is *verify*. |
| 3 | `.plan` import flattens a Survey `ComplexItem` into `simpleItems`; export writes plain waypoints. | Polygon/spacing are lost; breaks the "round-trips without information loss" acceptance for real QGC surveys. |
| 4 | Import copies `params` from the file as-is. | QGC writes NaN params (e.g. yaw) as `null`; Rust `f32` deserialization rejects `null`. The committed fixture has no NaNs, so this is untested (*verify* with a real QGC plan). |
| 5 | `itemsHash` includes the `current` flag. | The FC may report it differently on download → false "mismatch with FC". |
| 6 | Item editor shows the command read-only; no speed/hold/etc. | A survey needs at least speed changes and line markers. |
| 7 | Phase 1 acceptance is 100 waypoints. | Draped surveys reach hundreds/thousands of items. PX4 has a per-board item cap (*verify* for the target) and upload is stop-and-wait, slow on a 57.6 kbps radio. Densification (Phase 3) works against this. |
| 8 | Phase 3 acceptance "SITL deviates < 2 m AGL" mixes planner correctness, controller tracking and Gazebo terrain fidelity. | A failure cannot be attributed; also depends on the unresolved Gazebo setup (open question 8). |
| 9 | Phase 3 needs climb-rate limits, but vehicle parameters are Phase 5. | Minimal read-only parameter access must move earlier. |
| 10 | Phase 3 is one large phase. | Pure survey geometry has no DEM dependency and can ship and be field-tested earlier. |
| 11 | No geofence, sortie splitting, resume-after-RTL, mission stats, or pre-upload validation. | Multirotor endurance means a real survey is several sorties. |
| 12 | `core::height` has types and a `GeoidModel` trait but no implementation. | The 0.5 m datum acceptance cannot be checked. |
| 13 | The map only supports dragging existing waypoints. New waypoints come from the panel's Add button (map centre or selected item). | The main planning flow takes a detour. |
| 14 | Drag calls `updateItem` on every `MOUSE_MOVE`. | Every frame sets `dirty` and clears `fcMatches`; no undo granularity (one drag = hundreds of history entries). |
| 15 | Camera rotate/translate are disabled on drag start and restored only on `LEFT_UP`. | Releasing outside the canvas can leave the camera locked (*verify*). |
| 16 | Picking uses `camera.pickEllipsoid`. | Fine on a flat globe; wrong ground point on 3D terrain once a DEM is loaded. |
| 17 | No click-vs-drag discrimination. | Once click-to-add exists, panning the map would add waypoints. |
| 18 | The drag handler is installed on the viewer unconditionally; no per-view gating seen. | Waypoints may be draggable in the flight view (*verify*). |
| 19 | Toolbar has no "current tool" concept; the North button is a no-op. | No place to hang point-picking tools. |

---

## 2. Workstreams

### WS-A: Close Phase 1 properly (M)

1. Live SITL pass: 100-item upload/download; 10% loss via `tc netem` in the
   container; confirm Pause/Continue on PX4 v1.17.
2. Fix findings 1, 4, 5: convert existing items when the altitude mode
   changes; null-safe params on import; exclude `current` from the sync hash.
3. Command editor: dropdown with per-command parameter schemas
   (`NAV_WAYPOINT`, `LOITER_TIME`, `DO_CHANGE_SPEED`, `TAKEOFF`, `RTL`/`LAND`,
   a configurable DO action for line markers).
4. Undo/redo and mission stats (distance, estimated time, max altitude).
5. Lossless `.plan`: keep each `ComplexItem` as an opaque block carrying its
   original JSON; show it read-only; write it back unchanged on export unless
   the user explicitly converts it. Acceptance: a QGC plan with Survey,
   Corridor and geofence round-trips structurally identical.
6. Scale test: 1000-item upload; record time on a real radio; document the PX4
   item cap; if it bites, split into multiple uploads.
7. Map point-picking tools for Phase 1 (see WS-G).

### WS-B: Separate planning model from wire model (M, before Phase 3)

- Add `core::plan`: `PlannedMission { home, waypoints (datum-tagged Height),
  blocks, meta }` and `compile(frame_policy, home_amsl) -> Vec<MissionItem>`.
- **ADR-012**: plan and store in AMSL; convert to relative/AMSL at compile time
  using the FC's `HOME_POSITION`; do not use `GLOBAL_TERRAIN_ALT`. Keeps
  path A honest and removes the home-altitude ambiguity of relative frames.
- Acceptance: one planned mission compiles to different frames with identical
  geometry within a numeric tolerance.

### WS-C: Phase 3a, survey geometry without terrain (M)

- Inputs: polygon, azimuth, line spacing, tie-line spacing and azimuth,
  lead-in/lead-out length, turn radius, altitude, speed, direction mode
  (alternating or unidirectional, since heading error can matter for mag
  data).
- Outputs: line IDs (survey/tie numbering scheme), per-line length, total
  distance/time, and a **seq → line-ID table** so post-flight tools can
  segment lines from the ULog mission-seq topic without hardware markers.
- Implementation: local projection (UTM 10), `geo` crate for clipping (check
  `deny.toml` licences).
- Acceptance: golden-file geometry tests; sanity comparison against QGC Survey
  on the same polygon.

### WS-D: Phase 3b, terrain and height (L)

- `GeoidModel` implementations for EGM96 and CGG2013 grids; acceptance: known
  control points within 0.5 m.
- `ElevationSource` + GeoTIFF/COG reader (ADR-004); voids flagged, never
  interpolated.
- Draping: target `terrain + AGL`, forward-backward slope clamp from max
  climb/descent rate, then waypoint reduction with a vertical tolerance
  bounded by the item budget from WS-A6.
- DSM clearance check; AGL profile in the bottom panel.
- Minimal read-only parameters (`MPC_Z_VEL_MAX_UP/DN`, `MPC_XY_CRUISE`)
  pulled forward from Phase 5.

### WS-E: Operations and archive (M)

- Sortie splitting by endurance estimate; resume from line N after RTL
  (`MISSION_SET_CURRENT`).
- Pre-upload validation report: geofence containment, minimum AGL, item cap,
  climb-rate violations, distance from home.
- Mission archive (versioned JSON schema): parameters, DEM hash/version,
  geoid, line table, tool version, compiled-items hash. For determinism,
  quantize coordinates (1e-7°, 0.01 m) and avoid parallel or hash-order
  dependence. Acceptance: regeneration gives byte-identical `MissionItem`s on
  Windows and Linux.

### WS-F: Rewrite the Phase 3 acceptance (S)

Split into three tests:

1. **Offline**: compiled path vs DEM, max AGL error below a threshold (e.g.
   0.5 m); deterministic, no sim.
2. **SITL tracking**: fly the 3D path in a flat world, check vertical and
   cross-track error (tests execution, not terrain).
3. **Optional Gazebo heightmap** built from the same DEM, end-to-end, not a
   gate.

Over-limit climb rates must produce a warning at a stated threshold.

### WS-G: Map point-picking and interaction (M)

#### G1. Tool state machine

Add `mapTool: 'select' | 'add' | 'insert' | 'polygon' | 'home' | 'measure'`
to `stores/ui.ts`; default `select`; `Esc` always returns to `select`. New
`cesium/tools/`, each tool implementing
`activate / deactivate / onClick / onMove / onKey`. `installWaypointDrag`
folds into the `select` tool.

#### G2. Tools

| Tool | Behaviour |
| --- | --- |
| Select / edit (default) | Click selects; `Shift` multi-select; `Del` deletes; drag moves; segment midpoints show a "+" handle that inserts a waypoint there. |
| Add waypoint (`W`) | Click appends; `Esc`, right-click or double-click finishes. Ghost point under the cursor plus a rubber-band line from the last waypoint labelled with distance and bearing. Clicking on a segment becomes "insert into that segment". |
| Survey polygon (`P`) | Click vertices; close by clicking the first vertex or double-click; drag vertices; edge midpoints add vertices; self-intersection is flagged in the error colour and shows area and perimeter. From 3a, live survey-line preview. |
| Set Home / takeoff | Single click; not added to the waypoint list. |
| Context menu (right-click) | Add waypoint here / Set as Home / Centre here / Copy coordinates. "Fly to here" (Guided Goto) stays in Phase 5 with a mandatory confirmation. |
| Coordinate entry | Paste `lat, lon` (decimal degrees, DMS, UTM 10) → fly to it and optionally add. Useful for handheld GPS or RTK base coordinates. |

#### G3. Accuracy and feel

- **One pick function**, `cesium/pick.ts::pickLatLon(viewer, pos)`: use
  `globe.pick(ray)` in 3D with terrain, else `pickEllipsoid`; returns `null`
  for sky (callers must handle it); returns ground elevation once a DEM is
  loaded, for AGL conversion.
- **Click vs drag**: only press→release displacement ≤ 3 px counts as a click;
  otherwise it is camera motion and adds nothing.
- **Drag updates a preview only**, throttled with `requestAnimationFrame`; the
  store is committed once on `LEFT_UP`, producing exactly **one** undo entry
  and one `dirty` transition.
- **Camera-lock safety**: also listen to `window` `pointerup`, `blur` and
  `Escape` so rotate/translate are always restored.
- **Snapping**: to existing waypoints/vertices within 10 px; `Shift` locks
  bearing to 15° steps; optional snap to the survey-line spacing grid.
- **New-point altitude** inherits the previous waypoint's altitude (no
  hard-coded 50 m); in AGL mode show the ground elevation under the point.
- **Cursor readout** at the map bottom: lat/lon, plus ground elevation (AMSL)
  once a DEM exists.
- **Hint bar** per tool, e.g. "Click to add · Esc to finish".
- **Touch**: long-press = right-click; waypoint hit radius ≥ 44 px via
  `scene.pick(pos, w, h)` tolerance picking.
- **View isolation**: the flight view keeps only `select` (no edit), `measure`
  and centre; waypoints are not draggable or addable there. Editing a mission
  requires switching back to the planning view.

#### G4. Profile linkage (Phase 3b)

Hovering along the bottom AGL profile shows a synchronized crosshair on the
map; hovering the map track marks the same position on the profile.

#### G5. Code placement

- New: `frontend/src/cesium/pick.ts`, `cesium/tools/{select,add,polygon,home}.ts`,
  `stores/history.ts` (undo/redo).
- Changed: `stores/ui.ts`, `MapToolbar.tsx` (tool palette in the planning
  view, ≥ 44 px targets), `hooks/useCesiumViewer.ts` (ToolController wiring).
- i18n: new `map.tool.*` and hint strings, covered by `check:i18n`.
- Decision record: ADR-013 (tool state machine and the single pick function).

---

## 3. Phase mapping

| Phase / workstream | Map-tool content |
| --- | --- |
| Phase 1 (WS-A) | `select`, `add`, `insert`, context menu, coordinate entry, undo/redo, view isolation; fixes findings 14, 15, 17, 18 |
| Phase 3a (WS-C) | Polygon tool, live survey-line preview, grid snapping |
| Phase 3b (WS-D) | Terrain-aware pick, ground-elevation readout, map ↔ profile linkage |
| Phase 5 | Guided Goto with confirmation |

## 4. Suggested order

WS-A → WS-B → WS-C. WS-C can be field-tested while WS-D proceeds in parallel
with Phase 2 (RTK). WS-E and WS-F follow. Edit `DEVELOPMENT_PLAN.md` to split
Phase 3 into 3a/3b and to add ADR-012 (height policy) and ADR-013 (map tools).

## 5. Acceptance (automatable)

- Pick accuracy: Playwright with the dev `__mgViewer` handle injects clicks at
  several pixels; the new waypoint is within 1 m of `pickEllipsoid` for the
  same pixel (flat terrain), at three zoom levels.
- Click vs drag: a 50 px camera drag adds 0 waypoints.
- Drag granularity: one drag (≥ 100 mousemoves) → 1 undo entry, 1 store commit;
  one undo restores the original position.
- Camera lock: after releasing outside the canvas, rotate and translate still
  work.
- Polygon: a self-intersecting polygon is flagged and survey generation is
  blocked.
- Performance: ≥ 30 fps while dragging with 200 waypoints (matches §6).
- Touch: waypoint hit areas and tool buttons ≥ 44 px.
- View isolation: in the flight view, injected drag/add events leave the
  waypoint list unchanged.
- Plus the per-workstream acceptance listed above (`.plan` round-trip, 1000-item
  upload, datum < 0.5 m, archive byte-identity, offline AGL error).

## 6. Open questions

1. PX4 mission-item cap on the target board, and whether PX4 v1.17 honours
   `GLOBAL_TERRAIN_ALT_INT`.
2. Is unidirectional line flying needed as a default for the mag payload?
3. Flight view: forbid mission editing entirely, or allow dragging an
   un-uploaded draft? (Recommendation: forbid.)
4. Coordinate entry: decimal degrees only, or also DMS and UTM?
5. Do the tool shortcuts (`W`, `P`, `Esc`) conflict with the UgCS-style
   habits the layout is modelled on?
