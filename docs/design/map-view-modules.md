# MapView module split

Status: done (issues.md #29).

`MapView.tsx` grew to ~17 KB as the map gained the UAV model, trail, forward
projection, waypoint dragging, camera follow and 2D/3D morphing. The goal is to
split it along seams that already exist, without changing behaviour.

## Modules

- `frontend/src/cesium/constants.ts` — home position, trail/prediction/follow
  constants, the UAV model URI and nose-yaw offset.
- `frontend/src/cesium/uav.ts` — pure vehicle math: `groundSpeedMps`,
  `uavQuaternion`, `uavOrientation`, `projectAhead`. No React, no refs.
- `frontend/src/cesium/scene.ts` — WebGL probe, offline grid + OSM imagery,
  initial camera view; returns a configured `Viewer`.
- `frontend/src/cesium/entities.ts` — creation of the drone/trail/predict/home
  entities and the waypoint polyline + numbered points (layer math, no React);
  also `disposeWaypointLayer`.
- `frontend/src/cesium/waypoints.ts` — the screen-space drag handler
  (`LEFT_DOWN`/`MOUSE_MOVE`/`LEFT_UP`) committed to the mission store.
- `frontend/src/cesium/follow.ts` — `createFollowController`: dead-reckon
  advance and the hand-rolled camera-follow loop (see the comments on why
  `trackedEntity` is avoided).
- `frontend/src/hooks/useCesiumViewer.ts` — the React lifecycle binding
  (create on mount, tear down on unmount) exposing
  `{ initError, setSnapshot, setMission, goHome }`.

`MapView.tsx` is now composition only (46 lines): viewer lifecycle + store
wiring + render.

## Test protocol

The split is behaviour-preserving: each step is a pure move behind an import,
verified with `npm run typecheck`, `npm run build`, `npm run check:colors`,
`npm run check:contrast` and a Playwright headless-Chromium load of the built
app (no console errors, no 404s).
