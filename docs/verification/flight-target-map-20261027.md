# Live altitude and speed target verification — 2026-10-27

## Delivered

- Fly-map right-click actions: **Change altitude…** and **Change speed…**.
- Medium-risk press-and-hold confirmation; a click sends nothing.
- Altitude is entered as DTM AGL and converted in `core::height` to PX4 AMSL;
  missing terrain/geoid coverage and disarmed state fail closed.
- Live, survey and drape speed commands share PX4 v1.17 `param2` encoding.

## Automated evidence

- Core: 1,105 passed, 1 ignored.
- App: 275 passed, 8 ignored.
- `check-command-ux.mjs`: passed both headless and real-Cesium `--map` runs,
  including both typed intents and edit stability under live telemetry.
- Frontend production build, generated-type check, format and Clippy passed.

Screenshots are in `frontend/screenshots/commands/flight-target/`.

## Manual verification

1. Connect an armed PX4 with a loaded DTM, enter Fly, and right-click the map.
2. Open each target dialog; verify its live value, range message and hold action.
3. Hold to send altitude, confirm PX4 enters Hold and approaches the requested AGL.
4. Hold to send speed, confirm subsequent navigation uses the requested speed.

Steps 3–4 still require SITL/hardware execution; this record does not claim them.
