// Cesium map constants shared by the scene and the vehicle rendering.

/** Default initial position (Sidney, BC) for the camera, HOME fix and mock
 *  feed; operators override it in Settings -> Vehicle (`desktop/prefs.ts`). */
export const HOME_LAT = 48.6493
export const HOME_LON = -123.3982

// Recorded track (cesium/trail.ts). Fixes are kept by distance, not by count:
// a hover adds nothing and a survey line gets even spacing. 3 m x 30 000
// points is ~90 km of track, enough for a full battery of survey lines.
export const TRAIL_MIN_STEP_M = 3
export const TRAIL_MAX_POINTS = 30_000
/** Points per primitive chunk; only the newest chunk is ever rebuilt. */
export const TRAIL_CHUNK_POINTS = 200
/** Minimum gap between rebuilds of the newest chunk. */
export const TRAIL_TAIL_REBUILD_MS = 150
export const TRAIL_WIDTH_PX = 3

// Forward projection drawn ahead of the vehicle. Constant-velocity estimate
// from the live NED velocity; it is rebuilt from the current fix on every
// update, so any segment the aircraft has already flown is dropped and only
// the future track remains.
export const PREDICT_HORIZON_S = 120
export const PREDICT_STEP_S = 4
export const PREDICT_MIN_GROUNDSPEED_M_S = 1

// Largest gap the marker may be advanced past its last fix. Telemetry arrives
// at a few Hz, so snapping the marker (and the followed camera) to each fix
// makes the whole map move in visible steps; the reported NED velocity fills
// the gaps instead. The cap bounds how far a stale velocity can extrapolate.
export const DEAD_RECKON_MAX_S = 0.5

// Chase view applied when following starts, so enabling follow frames the
// vehicle instead of keeping whatever overview the user happened to be at.
// Once centred the user can orbit and zoom freely; follow keeps those.
export const FOLLOW_RANGE_M = 250
/** Waypoint-selection flyTo (#39): a closer, flatter look than follow. */
export const FOCUS_RANGE_M = 600
export const FOCUS_PITCH_DEG = -45
export const FOLLOW_PITCH_DEG = -35

// Shipped airframe (see `model/README.md` at the repo root). Cesium maps the
// asset's glTF axes so the nose (the camera gimbal, glTF +Z) lies on the
// body's +X axis. `uavQuaternion` rotates about +Z by -yaw, which leaves the
// +X nose 90 degrees clockwise of north when yaw is 0; passing `yaw - 90`
// puts the nose back on the reported heading.
export const UAV_MODEL_URI = '/model/scene-static.gltf'
