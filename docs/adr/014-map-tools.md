# ADR-014: Map tools — tool state machine and the single ground pick

- Status: Accepted (implemented with the polygon tool, WS-G)
- Date: 2026-10-08
- Supersedes: none. Related: ADR-013 (planning model; it reserved this number
  for the map-tool ADR).

## Decision

- The planning map has a small tool state machine in `stores/ui.ts`:
  `mapTool: 'select' | 'add' | 'polygon'`. `Esc` and leaving the planning view
  return to `select`; editing is gated to the planning view.
- One ground-pick function, `cesium/pick.ts::pickLatLon`, is the only way a
  tool turns a screen position into a ground point: `globe.pick` when real
  terrain is loaded, else `pickEllipsoid`, and `null` for the sky.
- The survey-boundary polygon lives in `stores/polygon.ts` (vertices +
  `closed`), drawn as a draft layer (`cesium/entities.ts`) and edited by the
  polygon tool in `cesium/waypoints.ts::installMapTools`: click adds a vertex,
  clicking the first vertex or double-click closes, dragging a vertex moves it,
  and `Esc` drops the draft.
- `frontend/src/mission/polygon.ts` mirrors `core::survey::validate_polygon`
  (area, perimeter, self-intersection) so the map can flag a bad boundary
  before Generate reaches Rust.

## Rationale

- A single pick function avoids the picking logic drifting apart across tools
  (finding 16) and gives every tool terrain-correct ground points once a DEM
  lands.
- Keeping the polygon in a store (rather than in the tool handler) lets the
  pattern panel and the map render the same draft, and lets the draft
  survive a tool switch so the operator can come back and keep editing.
- The geometry is mirrored in TS so the "self-intersecting polygon blocks
  Generate" acceptance is testable headless (`npm run check:polygon`) and
  fails fast, before a Tauri round-trip.

## Consequences

- Adding a tool is one `MapTool` variant plus a branch in `installMapTools`;
  the camera-lock/restore safety lives in one shared place.
- The draft polygon is a planning-view artifact: it renders only there and
  `Esc` discards it.
- A future terrain-aware drag will route through `pickLatLon` unchanged.