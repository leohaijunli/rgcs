// Cesium map constants shared by the scene and the vehicle rendering.

/** Default initial position (Sidney, BC) for the camera, HOME fix and mock
 *  feed; operators override it in Settings -> Vehicle (`desktop/prefs.ts`). */
export const HOME_LAT = 48.6493
export const HOME_LON = -123.3982

/** Recorded track length and decimation (keep every Nth fix). */
export const TRAIL_MAX = 512
export const TRAIL_EVERY = 2

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
export const FOLLOW_PITCH_DEG = -35

// Shipped airframe (see `model/README.md` at the repo root). Cesium maps the
// asset's glTF axes onto the body frame as +X -> north (+Y), +Y -> up,
// +Z -> east (+X), so the nose (the camera gimbal, glTF +Z) lies on the body's
// +X axis, 90 degrees clockwise of the +Y axis `uavQuaternion` calls the nose.
// Passing `yaw - 90` puts the nose back on the reported heading.
export const UAV_MODEL_URI = '/model/scene-static.gltf'
export const UAV_MODEL_NOSE_YAW_OFFSET_DEG = -90

/** Camera stand-off when framing a waypoint picked from a list, metres. */
export const FOCUS_HEIGHT_M = 600
/** Seconds for the frame-the-selected-waypoint flight. */
export const FOCUS_FLIGHT_S = 0.8
