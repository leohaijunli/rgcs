# UgCS Survey Operations Baseline

- **Language:** English | [简体中文](ugcs-survey-operations-baseline.zh-CN.md)
- **Recorded:** 2026-10-25
- **Purpose:** An independent, source-linked operating reference for comparing
  RGCS Survey functionality with UgCS.
- **Companion design:** [RGCS Survey workspace plan](plans/ugcs-survey-workspace-plan.md).
- **Status:** Reference baseline; video examples and current-manual behavior are
  identified separately. This is not a record of flights executed during this review.

## 1. Evidence and interpretation

The review used video metadata, English automatic captions, sampled video frames,
and the official manual. Automatic captions can misrecognize names, units, and
numbers. The procedures below prioritize named controls, chapter timestamps, and
cross-checked behavior. Exact software versions are listed only when the source
identifies them.

**Numerical settings are examples from the demonstrated aircraft and site.** They
are useful for reproducing a tutorial, not universal survey defaults. In
particular, camera overlap, magnetometer line spacing, vertical-rate limits, and
battery return thresholds describe different requirements.

The videos span 2016–2024. Current manual pages retrieved on 2026-10-25 describe
later behavior. A limitation demonstrated in an older video is not automatically
a limitation of current UgCS.

### 1.1 Video register

| ID | Source | Published | Length | Version / context |
| --- | --- | --- | --- | --- |
| V01 | [Simple Mission Planning](https://www.youtube.com/watch?v=yedoRF89icM) | 2017-09-19 | 6:33 | Official; UgCS 2.11; emulator |
| V02 | [Create an Area Scan route](https://www.youtube.com/watch?v=0qJbwQruDDs) | 2016-08-24 | 6:30 | Official; UgCS 2.9; emulator |
| V03 | [Terrain Following with Battery Change](https://www.youtube.com/watch?v=0w7GSJzU_Qw) | 2017-10-24 | 9:53 | Official; describes the UgCS 2.12 resume option; emulator |
| V04 | [Photogrammetry tool for land survey missions](https://www.youtube.com/watch?v=70pNNQMgDjk) | 2021-04-21 | 8:10 | Official; DJI Mavic 2 Zoom |
| V05 | [Step-by-Step Photogrammetry Planning](https://www.youtube.com/watch?v=gPSGU-uT0KU) | 2024-06-07 | 14:21 | Official; M350/H20; single grid and oblique double grid |
| V06 | [DJI Terrain Following Mission](https://www.youtube.com/watch?v=WP6OgLgvoq0) | 2022-01-31 | 18:05 | Indiana Drones; M300 + ROCK R2A LiDAR |
| V07 | [Autel EVO/EVO II Terrain Following](https://www.youtube.com/watch?v=NNHYtdLBubM) | 2021-10-20 | 6:08 | Official; UgCS 4.4; Android Autel Explorer |
| V08 | [Terrain Follow To and From Missions — workaround](https://www.youtube.com/watch?v=rALVSWCkq5g) | 2023-01-13 | 3:25 | Frontier Precision Unmanned; historical workaround |

The [Map import video](https://www.youtube.com/watch?v=SalCQoG6WUw) was private when
checked. Section 5 documents the current manual procedure instead. A specific
Connect PX4 video and the previously mentioned Industrial/magnetometry footage
were not available as identifiable video sources for this baseline.

### 1.2 Official manual register

| ID | Reference | Use in this baseline |
| --- | --- | --- |
| D01 | [Area scan](https://manuals-ugcs.sphengineering.com/docs/area-scan.md) | Basic/advanced parameters, actions, smoothing, double grid, partial calculation |
| D02 | [Altitude modes](https://manuals-ugcs.sphengineering.com/docs/altitude-types.md) | AMSL, AGL, Smart AGL, rangefinder, separate-waypoint terrain following |
| D03 | [Continue after a battery change](https://manuals-ugcs.sphengineering.com/docs/how-to-continue-the-route.md) | Current resume choices; DJI-specific qualifications |
| D04 | [Custom map layers](https://manuals-ugcs.sphengineering.com/docs/custom-map-layers.md) | Georeferenced imagery, XYZ, WMS |
| D05 | [Custom elevation layers](https://manuals-ugcs.sphengineering.com/docs/custom-elevation-layers.md) | DEM import, source priority, DTM/DSM, vertical datum behavior |
| D06 | [Autel KML export](https://manuals-ugcs.sphengineering.com/docs/how-to-export-route-for-autel-evo-in-kml-format-from-ugcs-autel-explorer-application-android-only.md) | Exact KML altitude and geometry options |
| D07 | [PX4 first connection](https://manuals-ugcs.sphengineering.com/docs/first-time-vehicle-connection-1.md) | Commissioning, transport, vehicle/profile association |
| D08 | [PX4 execution specifics](https://manuals-ugcs.sphengineering.com/docs/mission-execution-specifics-1.md) | Platform-dependent actions and trajectory behavior; version checking required |

## 2. Common operating model

The recurring workflow is:

1. Create/name the mission and add the vehicle.
2. Create a route and associate an aircraft profile.
3. Define the survey boundary or import geometry.
4. Select a survey tool and enter its geometry, altitude, speed, and payload parameters.
5. Add/reorder the route's start, survey, transit, calibration, and end elements.
6. Calculate the route; inspect both the 3D path and elevation profile.
7. Resolve route/profile limits and inspect the duration and item count.
8. Upload to the intended aircraft, then execute using that platform's controls.
9. Monitor position, altitude, link, and battery; interrupt and resume when required.

Important distinctions:

- **Survey boundary** specifies the area to cover. It is not a complete flight path.
- **Route segments** retain their purpose and parameters before they are expanded
  into individual flight points.
- **First waypoint, actual takeoff position, Home, return waypoint, and landing
  point** are related but separate concepts.
- **Area Scan** specifies height and line spacing directly. **Photogrammetry**
  derives geometry from a camera profile, GSD, and overlap.
- A successful upload and an accurate measurement are different outcomes. These
  videos do not establish a complete line-quality or magnetic-data validation system.

## 3. Video operating procedures

### V01 — Basic route, segment insertion, and actions

**Starting context:** UgCS 2.11 with a quadcopter emulator.

| Time | Operation | Expected visible result |
| --- | --- | --- |
| 00:22–00:41 | Menu → Create new mission; click the mission title, rename, and confirm. | A named mission is active. |
| 00:41–01:47 | Add vehicle; create a route from scratch; name it and choose the matching vehicle profile. Review Home, altitude, trajectory, and failsafe settings. | The route card and segment tools appear. |
| 01:47–02:24 | Place the first waypoint near the actual takeoff location with Shift-click or double-click. Drag its ground handle to move it; adjust its height handle or numeric altitude. | Coordinates and height follow the edited point. |
| 02:24–02:55 | Choose Stop and turn and, in the example, 6 m/s. Add outbound and inbound waypoints. | Subsequent points inherit the current editing settings. |
| 02:55–04:06 | Delete the inspection waypoint. Select the following segment, enable insertion before that segment, and place a Circle. Edit its center and radius; the example uses 50 m. | A circle occupies the intended place in the ordered route, rather than being appended at the end. |
| 04:06–04:49 | Add camera Start recording before the circle and Stop recording after it. Add a 5 s Wait and move it before Start recording. | Each segment has an ordered action list. |
| 04:49–05:31 | Select all segments and change altitude. Open Show elevation. | A shared edit affects the selected segments; the profile exposes AMSL/AGL extrema, distance, duration, and point count. |
| 05:31–06:10 | Upload, confirm, then use Arm and Auto mode in the emulator. | Upload feedback precedes route execution. |

**Baseline lesson:** Generated survey geometry must coexist with individually
editable route segments and ordered actions. Insertion position is an explicit
part of the editing operation.

### V02 — Area Scan with explicit height and spacing

| Time | Operation | Expected visible result |
| --- | --- | --- |
| 00:10–00:48 | Create a mission, add the emulator, and create a route with its profile. | A route is ready for planning. |
| 00:48–01:34 | Select Area Scan rather than Photogrammetry when specifying height directly or using a payload without a camera model. | Height and side distance are the primary geometric inputs. |
| 01:34–01:53 | Draw the survey polygon with Shift-click/double-click. | Survey passes are generated inside the boundary. |
| 01:53–03:07 | Set speed, turn type, altitude reference, flight height, and side distance. The example uses 5 m/s, Stop and turn, AMSL, and 20 m side distance. | The path reflects explicit metric spacing and altitude. |
| 03:07–03:54 | Configure action execution and a distance trigger; the example uses 10 m. | Trigger behavior is associated with the survey element. |
| 03:54–04:38 | Insert a waypoint before Area Scan at the drone's location; adjust its height. | The selected survey entry is near the approach point. |
| 04:38–05:40 | Calculate and inspect the profile. The example discovers low clearance, raises the survey's AMSL from about 1,650 to 1,675 m, and recalculates. | A constant AMSL route is visibly different from constant AGL; the revised example shows roughly 40–46 m AGL. |
| 05:40–06:06 | Upload, arm, and enter Auto mode. | The calculated mission is executed. |

The video's rough spacing-to-height suggestion is not a payload-independent
survey specification. For magnetic surveys, line spacing follows the measurement
objective; for imagery it follows footprint and overlap requirements.

### V03 — Terrain following and battery-change continuation

| Time | Operation | Expected visible result |
| --- | --- | --- |
| 00:43–01:48 | Create/name the mission, add the vehicle, and create an Area Scan route. | The route uses the intended aircraft profile. |
| 01:48–02:30 | Select Stop and turn, 20 m flight height, AGL, and 20 m side distance; draw the area. | The planned altitude varies with the terrain. |
| 02:30–03:27 | Inspect the fence-radius calculation error. Open Profiles → Edit → Routing, adjust the example profile's 500 m radius to 1,000 m, save, and return. | Route calculation is re-evaluated against the profile. These are historical demonstration limits. |
| 03:27–04:14 | Open Show elevation and review terrain, planned altitude, AGL variation, duration, and point count. | The example's approximately 23 min route exceeds its configured flight duration. |
| 04:14–04:50 | Change the line direction, approximately 60° in the example, to align with the area's longer edge. | Recalculation reduces the example's estimated duration to about 22 min. |
| 04:50–05:21 | Insert a waypoint before the survey near takeoff. | The route enters at a nearby survey corner. |
| 05:21–06:16 | Inspect Home in route parameters; choose explicit Home if needed and place it. | The intended return location is represented separately from the survey entry. |
| 06:16–07:30 | Upload and execute. Before returning, record the last reached point; the demonstration uses waypoint 46. Command Return home. | The operator retains a continuation reference before leaving the route. |
| 07:41–08:29 | Land, replace the battery, restart/reconnect, then choose Change start point and select waypoint 46. | A start flag marks the selected continuation point. |
| 08:29–09:17 | Upload again. Choose whether to fly through the first waypoint before the new start point; execute the route. | The drone follows the selected entry policy and continues rather than repeating the entire route. |

This is **waypoint-based continuation**. It does not demonstrate that an entire
measurement line was completed or that its acquired data passed QC. The battery
percentage shown in the example is not a generally applicable return threshold.

For current behavior, D03 lists Continue route, Start from beginning, Start from
waypoint, and Fly through the first waypoint. Its automatic interruption-memory
description is qualified for DJI aircraft. A direct connection to a resumed point
still needs its own terrain and trajectory assessment.

### V04 — Photogrammetry, camera profiles, and complete route endpoints

| Time | Operation | Expected visible result |
| --- | --- | --- |
| 00:13–00:42 | Add/name a route and select the DJI Mavic 2 Zoom profile. | Aircraft-specific route settings are active. |
| 00:42–01:21 | Select Photogrammetry; draw border points; press Enter; refine the boundary in 3D. | A generated grid follows the edited polygon. |
| 01:21–01:51 | Adjust line direction, speed, and turn type. The example uses 5 m/s and Stop and turn. | The grid order and estimated flight behavior change. |
| 01:51–03:01 | Select the camera. If missing, use Main menu → Payloads → Create new, enter camera parameters, then attach the payload to the vehicle profile. | The camera becomes selectable for this aircraft. |
| 03:01–03:54 | Set GSD; the example uses 3 cm/pixel. Open Show elevation to inspect the derived height and summary. | Camera geometry and GSD determine flight height; the example reports roughly 80–86 m AGL. |
| 03:54–04:22 | Set forward and side overlap; the example uses 80%/80%, briefly reducing side overlap to demonstrate wider line spacing. | Overlap changes spacing and workload. |
| 04:22–05:22 | Choose a supported distance/time trigger. The demonstration uses automatic time triggering. Add camera attitude, set the UgCS downward tilt to 90°, and move it before capture. | Camera orientation precedes acquisition. |
| 05:22–06:09 | Select AGL and add the first waypoint at takeoff. | Moving the first point selects a nearby grid entry. |
| 06:09–06:55 | Add an end waypoint for return and, where appropriate, a separate Landing element. | Return position and automatic landing are explicit route elements. |
| 06:55–07:44 | Review area, footprint, points, passes, images, trigger interval, profile, and duration; upload and execute using the platform's controls. | The demonstrated summary includes 11 ha, 45 points, 13 passes, 352 images, and about 19 min. |

### V05 — Modern parameters, single/double grid, and oblique imagery

| Time | Operation | Expected visible result |
| --- | --- | --- |
| 00:19–01:15 | Add a route, select the M350 profile, choose Photogrammetry and the H20 camera. The route wizard also offers KML/CSV import. | The planning tool is associated with aircraft and payload. |
| 01:15–02:09 | Draw the polygon, press Enter, and drag vertices to refine it. | The generated grid updates with the boundary. |
| 02:09–02:52 | Inspect the default distance trigger and add a 90° downward camera attitude. | The action list captures payload setup as well as acquisition. |
| 02:52–04:02 | Add the first waypoint at takeoff; observe entry-corner selection. Add an end waypoint near the same location. | The survey becomes part of an outbound/survey/inbound route. |
| 04:02–04:43 | Inspect the profile; reduce AGL tolerance from 3 m to 1 m. | More waypoints describe the terrain-following path. |
| 04:43–06:22 | Adjust GSD, speed, overlap, turn type, and altitude mode. The video discusses Smart AGL and demonstrates AGL comparison. | Resolution, workload, trajectory, and profile respond together. |
| 06:22–07:31 | Set direction with the map arrow or a numeric angle. Demonstrate a 20 m overshoot and a distinct overshoot speed, then remove the overshoot for this route. | Turn space and transit speed are independently controllable. |
| 07:31–08:53 | Choose action execution: Every point, At start, or Forward passes. Inspect the option to add a waypoint at every photo location. | Capture can be restricted to measurement passes; extra capture points change item count. |
| 08:53–10:08 | Open the route summary/log. Review area, camera footprint, pass count, image count, and trigger spacing; change GSD to see the estimates update. Name the route Single grid. | Planning cost and acquisition output are visible before upload. |
| 10:08–11:16 | Duplicate the route, rename it Double grid, hide the original, enable Double grid, and change camera tilt to 45°. | A second orthogonal grid with oblique camera attitude is created without losing the first route. |
| 11:16–12:20 | Adjust oblique overlap/tilt; inspect automatic trigger distance and the manual override; inspect time triggering as an alternative. | Payload settings remain editable after duplication. |
| 12:20–14:09 | Attach an existing custom camera to the aircraft profile or create a new payload. Select it back in the route and adjust GSD. | Changing the camera recomputes the derived altitude and route summary. |

The example changes the nadir overlap to 80% forward/70% side and discusses
80%/80% for oblique capture. These are demonstrated settings. The baseline does
not normalize the video's distance/time-trigger commentary into a universal
recommendation for all aircraft.

### V06 — Office-to-field LiDAR operation

| Time | Operation | Expected visible result |
| --- | --- | --- |
| 01:38–02:38 | Prepare UgCS on the laptop and UgCS on the M300 controller; use a shared local network/hotspot. | The planning and aircraft-control devices can communicate. |
| 02:38–04:19 | Draw the AOI in Google Earth and Save place as KML. Inspect terrain, access, visibility, and a practical staging location. | The imported survey boundary and field access plan are prepared before travel. |
| 04:19–06:55 | Edit the M300 vehicle profile: routing limits, vertical rates, and default speed. | Route feasibility uses the selected aircraft configuration. |
| 06:55–07:42 | Create/associate the demonstrated R2A payload profile. | Payload/overlap settings are available for route creation. |
| 07:42–09:01 | Import the KML as a LiDAR Area and select the aircraft/payload setup. The example chooses 50 m height, 33% side overlap, and 5 m/s. | A terrain-following route is generated from the external AOI. |
| 09:01–10:29 | Adjust direction and turning parameters, then inspect Show elevation. | Both horizontal turns and the vertical path are reviewed. |
| 11:09–12:14 | Inspect the DJI Pilot aircraft-side altitude/distance settings as well as the UgCS profile. | The workflow exposes that planner limits and aircraft limits are separate configurations. The author's limit changes are site/platform examples. |
| 12:38–13:44 | In the field, connect laptop, controller, and aircraft in the demonstrated sequence; verify battery, GNSS, telemetry, and RC status. | The actual aircraft position appears in the plan. |
| 13:44–14:34 | Place takeoff at the aircraft, reorder the segments so takeoff is first, and add the landing location. | Office planning is reconciled with the actual field setup. |
| 14:37–15:27 | Start the LiDAR, upload from the beginning, enter Auto, and use the demonstrated figure-eight calibration maneuver. | Acquisition and mission execution have identifiable start/calibration steps. |
| 15:38–17:21 | Monitor visual position, controller FPV, the laptop's 3D route, altitude, speed, distance, and battery. | The operator can correlate aircraft state with the planned route. |

The author explicitly asks for more prominent battery information at 16:25.
The narration is uncertain about one turning parameter's units around 09:41;
this baseline intentionally does not promote that numerical value to a preset.

### V07 — Autel export/import and action reconstruction

| Time | Operation | Expected visible result |
| --- | --- | --- |
| 00:31–01:20 | Choose the Autel EVO II profile and prepare the terrain/map context. | A compatible route is ready for planning. |
| 01:20–02:03 | Create takeoff/approach, the photogrammetry area, and return geometry. | The entire intended trajectory is represented. |
| 02:03–02:34 | Change distance triggering to time triggering; note the calculated interval. The example uses 2 s. | Acquisition timing is known for recreation in the receiving app. |
| 02:34–03:21 | Route parameters → Export KML. Choose **Relative to the ground under the 1st waypoint** and **As a linestring**. Save. | The exported coordinates have the receiving application's expected geometry and height reference. |
| 03:21–03:58 | Check that the actual launch location is near the first waypoint used as the altitude reference. | The receiving system's initialization reference is reconciled with the export reference. |
| 03:58–04:45 | Copy the file to Android, open it in Autel Explorer, and inspect the trajectory and point heights. | The path is imported; this does not establish that the payload actions were imported. |
| 04:45–05:45 | Edit camera pitch for nadir imagery using the app's angle convention; add time-lapse capture at the recorded interval. Check that heights remain unchanged. | The receiving application has both the path and the required camera behavior. |
| 05:45–05:57 | Connect the aircraft and upload for execution. | The reconstructed mission is ready for the platform's flight procedure. |

D06 confirms the two KML export options and the Android workflow. The relevant
functional baseline is **explicit geometry, height-reference, and action fidelity**
across interchange, rather than treating KML as a complete mission container.

### V08 — Historical outbound/inbound terrain-following workaround

| Time | Operation | Expected visible result |
| --- | --- | --- |
| 00:20–01:32 | Create a route with takeoff, a remote LiDAR area, and landing near takeoff. | The example exposes problematic direct approach/return segments. |
| 01:47–02:08 | Insert a very narrow Corridor between takeoff and the survey start, producing one flight line. | The approach is calculated through a terrain-following survey tool. |
| 02:08–02:30 | Insert a similar narrow Corridor after the survey toward landing. | The return also receives terrain-following points. |
| 02:30–03:20 | Inspect the resulting full route. The author notes that the payload records during these corridors. | The geometric workaround also changes acquisition behavior. |

**Version correction:** D02 now documents separate-waypoint **AGL - Terrain
following** and Smart AGL behavior for side movements/overshoots. V08 is evidence
for the importance of complete-route terrain coverage and transit/acquisition
separation, not proof of a current UgCS feature omission.

## 4. Parameter semantics baseline

| Parameter / control | Meaning and observable effect | Source |
| --- | --- | --- |
| Flight height + altitude mode | The number needs a reference: AMSL, vertical AGL, Smart AGL, or a supported rangefinder mode. | V02, V03, D02 |
| Side distance / line spacing | Direct metric distance between survey passes in Area Scan. | V02, D01 |
| GSD + camera profile | Camera geometry and requested image resolution determine nominal survey height. | V04, V05 |
| Forward / side overlap | Along-track capture spacing and cross-track pass spacing; affects duration and image count. | V04, V05 |
| Direction angle | Orientation of survey lines, distinct from rotating/translating the complete segment. | V03, V05, D01 |
| First waypoint / entry | Influences the selected entry corner; remains part of the approach geometry. | V02–V05 |
| Turn type | Platform-dependent stop/continuous-turn behavior; a displayed curve alone is not a flight-controller contract. | V01–V06, D08 |
| Overshoot and overshoot speed | Additional length at line ends and a distinct speed for those parts. | V05, D01 |
| AGL tolerance | Controls additional terrain-following points and therefore item count. It is not a guarantee of actual flight-height accuracy. | V05, D01 |
| Trajectory smoothing | Current Area Scan exposes Max slope and Minimal gap. | D01 |
| Action execution | Every point, At start, or Forward passes; D01 also exposes No action at the last point. | V05, D01 |
| Double grid | Adds a second grid at 90°; camera attitude remains independently configurable. | V05, D01 |
| Additional capture waypoints | Adds explicit points at photo locations when needed. | V05 |
| Allow partial calculation | Current Area Scan can calculate partial output when altitude, data availability, fence, or NFZ constraints exclude parts. Partial output must be distinguished from complete coverage. | D01 |
| Move / rotate segment | Move with the segment's center handle; rotate with its rim. | D01 |
| Route summary | Area, path distance, ETA, points, passes, footprint/captures where applicable, and altitude extrema. | V01–V05 |

Smart AGL is described by D02 as distance to the nearest surface **on the flight
line**, rather than ordinary vertical AGL. The same page documents license and
aircraft restrictions. Rangefinder following and DEM-derived waypoint following
are distinct execution modes.

## 5. Manual-backed supplementary procedures

### 5.1 Map imagery

From Map options → Map layers → Map:

1. **Georeferenced image:** Add → name → Geoserver → Create → Upload a
   georeferenced TIFF.
2. **XYZ service:** Add → name → External → enter Remote Source → Create;
   choose Base/Overlay ordering.
3. **WMS:** Add → name → External WMS → enter service URL → select layer →
   Create; choose Base/Overlay ordering.

Expected result: the selected imagery is visible and spatially aligned. Imagery
visibility alone does not establish that elevation data is available for planning.

### 5.2 Elevation data

From Map options → Map layers → Elevation:

1. Add and name the elevation source.
2. Upload a georeferenced elevation raster, or configure an external source URL.
3. Enable the source; set its priority in the list; use Focus to inspect coverage.
4. Identify the DTM/DSM role and inspect the boundary where sources meet.
5. Recalculate the route and inspect its elevation profile.

D05 states that built-in SRTM uses EPSG:4326 horizontally and EGM96 vertically.
Imported elevations are **not converted between vertical datums** by UgCS. Mixing
datums can therefore produce a step at a source boundary. The manual also states
that inputs finer than 1 m/pixel are downscaled to 1 m/pixel. DTM drives route
calculation; DSM supports object-clearance assessment.

### 5.3 PX4 connection and compatibility

This section is manual-backed, not a reconstruction of an unavailable video.

1. Complete aircraft setup/calibration/testing through the PX4/QGroundControl
   workflow before connecting it to UgCS.
2. Verify unique vehicle identities and the intended connection endpoint.
3. Connect the paired telemetry radio or flight-controller USB to the machine
   running VSM; confirm that the OS exposes the serial connection.
4. Wait for the vehicle to appear; edit its name and assign the matching profile.
5. Confirm supported mission actions and the actual aircraft limits before execution.

**Version discrepancy to retain:** D07 calls the system-ID parameter
`SYSID_THISMAV`. The local PX4 v1.17 source defines `MAV_SYS_ID` in
`src/modules/mavlink/mavlink_params.c`; the former name must not be copied into
PX4-specific instructions without checking the target firmware. Similarly, D08's
trajectory/heading restrictions are not treated as experimentally verified
behavior of every later PX4 release.

## 6. Stable capability IDs for RGCS comparison

The companion design uses these IDs, so acceptance can refer to operations rather
than to an imprecise request to “look like UgCS.”

| ID | Capability baseline | Primary evidence |
| --- | --- | --- |
| BL-01 | Named missions/routes, aircraft/profile association | V01, V03, V04 |
| BL-02 | Draw/edit/import survey geometry | V02, V05, V06 |
| BL-03 | Ordered segments; insert before/after; replace, move, and rotate | V01, V06, D01 |
| BL-04 | Direct height/spacing/direction/speed Area Scan | V02, D01 |
| BL-05 | Camera/payload profiles; GSD/overlap-derived planning | V04, V05 |
| BL-06 | Explicit altitude modes and terrain-following semantics | V03, V05, D02 |
| BL-07 | Terrain tolerance, smoothing, and resulting item count | V05, D01 |
| BL-08 | Turn behavior, overshoot length, and overshoot speed | V05, V06, D01 |
| BL-09 | Entry selection; actual takeoff, Home, return, and landing | V02–V06 |
| BL-10 | Ordered actions, trigger mode, action scope, last-point behavior | V01, V04, V05, D01 |
| BL-11 | Recomputed route summary and elevation profile | V01–V06 |
| BL-12 | Duplicate, rename, and hide/show route variants | V05 |
| BL-13 | Orthogonal double grid and oblique camera attitude | V05, D01 |
| BL-14 | Map/DEM import, source priority, DTM/DSM distinction | D04, D05 |
| BL-15 | Aircraft/profile constraints, NFZ effects, partial calculation | V03, V06, D01 |
| BL-16 | Upload feedback, platform execution, live telemetry | V01, V03, V06 |
| BL-17 | Interruption, battery change, selected restart, first-point option | V03, D03 |
| BL-18 | Interchange with explicit geometry/height/action fidelity | V06, V07, D06 |
| BL-19 | Survey segments for approach/return and complete-route terrain coverage | V08, D02 |
| BL-20 | LiDAR-area workflow and inserted calibration maneuver | V06 |

### 6.1 Suggested comparison scenarios

For each scenario, record aircraft/profile/version, input geometry, parameters,
generated summary, elevation profile, and exported/executed result. These are
**future comparison scenarios**, not tests run during this documentation review.

1. Draw an irregular polygon; edit a vertex, translate and rotate the segment,
   change line direction, and repeat with imported boundary geometry.
2. Move takeoff to another corner; verify entry selection and all approach/return legs.
3. Change AGL tolerance over a ridge; compare generated count and minimum clearance.
4. Set distinct survey and overshoot speeds; inspect the executed speed contract.
5. Compare Every point, At start, and Forward passes action execution.
6. Duplicate a single-grid route; enable double grid and change camera attitude.
7. Substitute a camera profile; verify GSD, spacing, trigger interval, and image count.
8. Change DEM priority/datum; inspect the source boundary and regenerated profile.
9. Trigger a route limit or a missing-elevation region; distinguish complete,
   partial, and failed calculation.
10. Interrupt halfway through a line, change the battery, and resume with both
    direct and first-waypoint entry choices; inspect the resulting flown coverage.
11. Export/import the route through another application; check heights and
    explicitly account for any actions that need reconstruction.

## 7. Evidence storage

The local research artifacts are in `/tmp/opencode/ugcs-review/`:

- `<video-id>.metadata.txt`: publication date, description, and chapter metadata.
- `<video-id>.transcript.txt`: de-duplicated automatic captions with timestamps.
- `<video-id>.mp4`, caption VTT files, contact sheets, and selected keyframes.
- `build_evidence.py`: caption and frame extraction helper.

Temporary media is not required to use this document: the stable video IDs,
source URLs, timestamps, operating steps, and qualifications are recorded above.
If a source changes, revise the relevant capability row and preserve its previous
version context rather than silently replacing the historical procedure.
