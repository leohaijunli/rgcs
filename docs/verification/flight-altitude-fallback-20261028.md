# Live altitude AMSL fallback — 2026-10-28

## Behavior

Opening **Change altitude…** without a ready DTM sample uses **Target AMSL (m)**
and the FC's reported current AMSL. Missing, loading, uncovered/nodata and
mesh-only terrain all take this path. A later DEM load preserves the entered
value and its units; reopening with a ready DTM selects AGL.

The native intent uses `FlightAltitudeTarget`: AGL is converted through
`core::height`, while AMSL carries an explicit `AMSL_EGM96` `Height` and is sent
unchanged in `MAV_FRAME_GLOBAL`. The armed/mode and owned-target guards apply to
both references. Invalid datums, non-finite heights and MAVLink numeric overflow
are refused before dispatch.

## Automated evidence

- Native PTY command fixture: 1,250 m and −25 m AMSL reach the command's
  `param1` exactly with `param2 = 0`, without a loaded DEM. Invalid heights and
  disarmed requests produce zero writes; explicit AGL still needs a DTM.
  All 17 native flight-command fixtures pass.
- `check-command-ux.mjs`: missing/loading/nodata/mesh-only fallback, reported
  current AMSL, ready AGL, 1–120 m AGL range, stable edits after DEM arrival,
  typed payloads and armed/hold-to-send behavior passed with zero page errors.
- Production frontend build, generated-type consistency, i18n, Rust format and
  Clippy (`--all-targets -- -D warnings`) checks passed.

Screenshots:

- [AMSL fallback](../../frontend/screenshots/commands/altitude-fallback/flight-target-altitude-amsl.png)
- [Ready DEM / AGL](../../frontend/screenshots/commands/altitude-fallback/flight-target-altitude-agl.png)
- [Unarmed guard](../../frontend/screenshots/commands/altitude-fallback/flight-target-altitude-unarmed.png)

## Manual verification

1. Connect PX4, enter Fly and leave the DEM unloaded. Right-click the map and
   open **Change altitude…**. Check **Current AMSL**, **Target AMSL (m)** and the
   initial value against PX4's reported AMSL.
2. Enter an absolute altitude appropriate for the test location. Load a DEM
   while the dialog is open; confirm the value and AMSL label remain stable.
3. Reopen after the DEM is ready under the vehicle. Confirm the AGL labels and
   1–120 m input range; repeat outside coverage to confirm AMSL fallback.
4. In SITL, arm in Hold, hold to send an AMSL target and confirm PX4 enters/stays
   in Hold and approaches that absolute altitude. Repeat with a ready AGL
   target. A short click and an unarmed request must send nothing.

Physical altitude tracking in step 4 requires SITL/hardware verification; the
automated evidence above measures payload delivery and UI behavior.
