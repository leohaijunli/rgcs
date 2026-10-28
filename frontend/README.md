# Q-Mag React frontend

React/TypeScript, Cesium, Radix UI, shared design tokens, and English i18n.
See [ADR-010](../docs/adr/010-frontend-stack.md).

## Live altitude targets

Fly → right-click → **Change altitude…** uses DTM AGL when a ready DEM samples
under the UAV. Otherwise it uses **AMSL**, seeded from the FC-reported absolute
altitude. The label, current readout and typed command share that reference.
The reference is fixed for the edit, so a later DEM load cannot reinterpret an
entered value. AGL accepts 1–120 m; AMSL accepts finite absolute heights,
including heights above 120 m and below sea level.

```bash
node scripts/check-command-ux.mjs http://localhost:1420/ /tmp/opencode/command-ux
```

Set `PLAYWRIGHT_MODULE` when Playwright is installed outside the frontend and
`FLIGHT_TARGET_SHOTS_DIR` to retain altitude screenshots separately. The check
covers missing/loading/nodata/mesh-only DEM, datum-tagged AMSL payloads, a ready
AGL sample, stable edits and the armed/hold-to-send guards. See
[verification and manual steps](../docs/verification/flight-altitude-fallback-20261028.md).

## Signal Inspector ULog history

Loading a `.ulg` opens **Full log** across the recording's entire time range.
Selected signals, including signals added afterward, load their history
independently of playback. Zooming, seeking and trace edits preserve the viewed
range; Follow restores the full extent. See the
[Inspector module guide](src/inspector/README.md) for regression commands,
screenshots and manual verification.

## RTK drawer

`components/RtkDrawer.tsx` offers **F9P Survey-In** and **Preconfigured RTCM
receiver** modes. Survey defaults are 2.00 m / 180 s, with both conditions
required and completion reported by the receiver. Native serial profiles own
automatic settings; valid older local preferences are imported on first use.
Manual edits are applied on Connect. Survey-In works without a vehicle link.

The selector labels FC, RTK-candidate and unknown devices, excludes known/in-use
FC ports and requires reselection when USB identity changes. During startup,
Cancel is available once a session exists; Disconnect stays available after
heartbeat loss. A failure is visible and retryable. Base-session/revision and
ordered link-status guards reject stale cache/events and late Start replies.

Base progress/identity, raw source freshness, valid RTCM freshness, locally
written correction throughput and rover-reported fix are separate. Vehicle
disconnect clears forwarding state and preserves the base. See the
[detailed TODOs](../docs/plans/rtk-base-and-air-ground-todos.md).

Run `npm run check:rtk -- http://localhost:5199/ screenshots/rtk` with Vite
running. The regression covers invalid inputs, offline survey, dual-target
progress, fresh corrections/rover separation, FC disconnect, Stop failure/retry,
changed USB identity, late cached sessions and cancellation before Start's
reply. Screenshots, visually checked at desktop/compact/Field sizes:

- [Settings, 1920×1080](screenshots/rtk/survey-in-settings-1920x1080.png)
- [Survey beyond the minimum time, 1024×640](screenshots/rtk/survey-in-progress-1024x640.png)
- [Ready base / FLOAT rover](screenshots/rtk/survey-in-ready-1920x1080.png)
- [Field/light, stale RTCM with live source data](screenshots/rtk/survey-in-field-light.png)

Manual packaged-app review:

1. Open the RTK pill (More at compact widths). Pause auto for manual input checks. Check device labels, defaults
   and the disabled FC option. Invalid accuracy or fractional/zero duration
   must disable Connect.
2. Select the intended F9P, Connect, and check real identity/configuration
   reports. Cancel during identification; reconnect and observe the survey.
3. Check elapsed time may exceed its minimum while estimated accuracy is still
   above target. After completion inspect valid RTCM age and local write counts.
4. Disconnect/reconnect the FC: the base survey/session continues, forwarding
   resumes with fresh data. Compare rover state with independent GNSS evidence.
5. Unplug/replug the base: an enabled saved identity reconnects under a new
   session; a paused role remains paused. A different device requires selection.
6. Repeat at 1024×640, 1920×1080 and Field/light; check wrapping, scroll access,
   pinned Disconnect and keyboard Escape/focus containment. Exit the app and
    verify the receiver port is released.

## Automatic links and RTK/GPS HUD

`SerialAutoControls` uses the native revisioned policy in Settings → Connection
and the RTK drawer. Automatic linking defaults on for eligible devices. Manual
Connect/Disconnect, serial/baud and receiver-mode controls remain available.
`useSerialSelection` binds the manual choice to the attachment ID, including
same-path device replacement; explicit reselection is required after a change.

`RtkGpsDashboard` sits at the drawer top: large FIXED/FLOAT/3D solution status,
satellite count, dimensionless HDOP/VDOP, an RTCM-age ring, local correction-TX
history (up to 60 samples/seconds) and independent time/precision progress bars.
LIVE/STALE/NO DATA are explicit. Full bars do not imply receiver validity;
TX does not imply rover receipt. Receiver/transport changes clear history.

```bash
npm run check:serial-auto -- http://localhost:5199/ screenshots/serial-auto
```

The browser fixture verifies native startup ownership, default-on policy, legacy
settings import, manual precedence, port-ID invalidation, persistent pause,
Resume, old snapshots, stale FIXED and session-scoped HUD history.

- [HUD, desktop](screenshots/serial-auto/rtk-gps-hud-1920x1080.png)
- [HUD, compact](screenshots/serial-auto/rtk-gps-hud-1024x640.png)
- [HUD, Field/light](screenshots/serial-auto/rtk-gps-hud-field-light-1366x768.png)
- [Manual FC controls](screenshots/serial-auto/fc-manual-controls-1920x1080.png)
- [Automatic policy and manual connection, Field/light](screenshots/serial-auto/fc-manual-field-light-1366x768.png)

For manual verification, compare HUD values to the independent receiver, remove
telemetry and correction inputs separately, check stale indicators and history
reset, and scroll to the retained manual controls. The
[20-case hardware checklist](../docs/verification/serial-rtk-hardware-checklist.md)
provides setup, timing, expected results and evidence requirements.

## Shell and branding

`components/TopBar.tsx` keeps the title/project entry, tools, Settings,
workflow, and vehicle status on one row. `WorkflowNav.tsx` provides the five
numbered stages, Previous/Next, keyboard navigation, and focus restoration.
Inspector and Motor test appear inline at ≥1600 px and in Tools below that
width. About is in Settings. Tools contains no map pop-out entry.

`AppLogo.tsx` shares three SVG treatments of the supplied `Q-Mag-logo.svg`:
`full`, `wordmark`, and `mark`. `branding.css` gives the logo a transparent,
theme-aware surface. The main toolbar displays the Q mark at ≤1366 px. Tool
headers use a 24 px wordmark/mark before their titles, with a small separator;
Logs and Settings always use the compact mark. Logos are labelled images,
not additional tab stops.

`CompanyLogo.tsx` uses the supplied Quaternion Aerospace assets from
`public/brand/` on StartPage and Settings → About. The PNG is used in dark and
night-red themes; the JPG in light theme. The neutral backing preserves the
source artwork's readability. Theme colors are defined in the shared design
system. Persistent storage uses the existing `maggcs.*` keys for compatibility
with saved preferences and workspaces.

## Verification — 2026-10-23

Run from `frontend/` with Vite at `http://localhost:5199/`:

```bash
npm run build
npm run check:colors
npm run check:i18n
npm run check:contrast
node scripts/check-topbar.mjs http://localhost:5199/ screenshots/u2
node scripts/e2e-shell.mjs http://localhost:5199/
node scripts/check-startpage.mjs 'http://localhost:5199/?start&nomap' screenshots/start
node scripts/check-field-mode.mjs http://localhost:5199/ /tmp/opencode/field-shots
node scripts/check-inspector.mjs http://localhost:5199/inspector.html screenshots/inspector
node scripts/check-params.mjs http://localhost:5199/params.html screenshots/params
node scripts/check-params-connection.mjs http://localhost:5199/params.html screenshots/params/connection
node scripts/check-motor-controls.mjs http://localhost:5199/actuator.html screenshots/motor-test
node scripts/check-logs-delete.mjs 'http://localhost:5199/?nomap' screenshots/logs
```

These checks use browser mocks or isolated IPC stubs. Acceptance includes:

- Complete main-window status and persistent RTL/Hold at 1024, 1280, and
  1920 px widths; a 32 px normal-mode / 44 px Field-mode control floor.
- Keyboard workflow selection, Previous/Next boundaries, Tools arrow
  navigation, Escape focus restoration, and responsive shortcut placement.
- Q-Mag title, responsive logos, removed map menu entry, and live company-logo
  theme switching in About.
- A reachable create form and no-project entry at 1024 × 700 with five recent
  projects and Field mode enabled.
- Inspector at 800 × 480, Parameters at 900 × 600, and Motor test at
  1024 × 640, as well as both desktop sizes. Header/logo bounds were also
  measured in dark, light, and night-red themes.
- Parameters: the firmware-version IPC contract, tagged numeric values,
  hardware-specific catalog entries, search virtualization, same-endpoint
  reconnect recovery and rejection of delayed old replies. See the
  [module verification](src/params/README.md) for read-only serial acceptance
  and native desktop review steps.

### Screenshots

| Surface | Capture |
| --- | --- |
| Main, wide / compact | [1920 px](screenshots/u2/topbar-1920.png), [1024 px](screenshots/u2/topbar-1024.png) |
| Workflow / Tools | [Workflow](screenshots/u2/workflow-1920.png), [Tools](screenshots/u2/tools-1920.png) |
| About, dark / light | [Dark](screenshots/u2/about-dark.png), [Light](screenshots/u2/about-light.png) |
| Startup / long Field-mode form | [Startup](screenshots/start/start-page.png), [Scrollable form](screenshots/start/start-field-light.png) |
| Inspector, minimum | [Dark](screenshots/inspector/inspector-800x480.png), [Light](screenshots/inspector/inspector-light.png) |
| Parameters, minimum | [Dark](screenshots/params/params-900x600.png), [Light](screenshots/params/params-light.png) |
| Motor test, wide / minimum | [Wide](screenshots/motor-test/motor-controls-1920x1080.png), [Minimum](screenshots/motor-test/motor-controls-1024x640.png) |
| Logs | [Logs dialog](screenshots/logs/logs-delete.png) |

### Manual review

1. Resize the main window through 1920, 1366, and 1024 px. Confirm that the
   wordmark becomes Q, the tools remain reachable, and status plus RTL/Hold fit.
2. Open Workflow with the keyboard. Use arrows, Home/End, Enter, Previous/Next,
   and Escape; check that the selected workspace and trigger focus agree.
3. Open Inspector, Motor test, Parameters, Logs, and Settings. Check the logo
   to the left of each title; resize each tool to its declared minimum and
   confirm that its primary controls remain reachable.
4. In Settings → Appearance, choose dark, light, and night-red. Check the logo
   and credits in About, then reopen tool windows to review their headers.
5. Enable Field mode. On StartPage with five recents, open New project and
   scroll from the logo to the create controls and Continue without a project.
