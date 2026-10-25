# Startup, Settings, and Tools: UI Assessment and Improvement Plan

- **Review date:** 2026-10-22
- **Status:** Motor Test, Parameters, Inspector, and StartPage deliveries
  landed on 2026-10-22. The compact TopBar, unified About, and Q-Mag branding
  followed on 2026-10-23; remaining Settings/diagnostics tasks stay deferred.
- **Scope:** The startup/project-selection page, main-window Settings, the Tools
  menu, and the layouts of the tools it opens.
- **Related assessment:** [Tools → Logs](logs-ui-review-plan.md).
- **Execution rule:** Resume one small task at a time. Present affected types,
  function signatures, and error types for confirmation before implementation.

The startup surface reviewed here is `StartPage`, the project's entry screen.
Its current purpose is selecting or creating a project. The assessment does not
assume an account-authentication workflow.

**Implementation update (2026-10-22): StartPage delivery.** Startup is no longer
a hard project gate:

- **S1 resolved:** the telemetry lifecycle surfaces (auto-connect, target
  change, alerts, heartbeat, errors) are a shared `LifecycleOverlays` mounted by
  both StartPage and the workspace, so the auto-connect timer now has visible
  cancellation UI on the start page.
- **No-project entry:** a *Continue without a project* action opens the
  workspace with `project === null`; closing a project returns to StartPage.
  The TopBar's `No project` pill still opens the project picker.
- **Create form simplified:** the wizard asks only for a name. CRS/datum use the
  project defaults and the project is created under `<app data>/projects` with a
  unique folder name, with an optional *Change location…*. StartPage and
  ProjectHome share one `ProjectWizard`, removing the duplicated form (S4's
  labelled-metadata recommendation is superseded by this simplified contract).
- **Project deletion:** recents gain *Remove from list* (localStorage only) and
  *Delete from disk* (typed project-name confirmation). The backend validates
  `project.json` before removing the tree and refuses an open project.
- **S3 (import) and S6 (availability/last-opened) remain open** — import is still
  ProjectHome-only; recents show no availability or last-opened metadata.

## 1. Executive assessment

The application has useful individual components, but its entry points,
configuration behavior, and tool-window layouts need stronger consistency.

The highest-value improvements are:

1. **Make startup behavior visible and recoverable.** Background auto-connect
   currently runs while its countdown/confirmation UI is absent from StartPage.
2. **Make settings trustworthy.** Connection drafts disappear on tab changes,
   blank coordinates can become a saved `(0, 0)` area, and persistence failures
   are not surfaced.
3. **Fix measurable layout defects.** Parameter rows do not share the header's
   column positions; the reviewed Motor Test layout overflows its viewport.
4. **Apply shared preferences across windows.** Field mode is not applied at
   startup in the three main tool windows, and open windows do not follow a
   main-window theme change.
5. **Standardize navigation and visual hierarchy.** Use common titles, spacing,
   status vocabulary, action sizing, and keyboard behavior across Settings and
   Tools, while retaining task-appropriate workspaces.

Recommended order: **behavioral correctness → layout correctness → shared
presentation rules → workflow and visual refinement**.

## 2. Evidence and review conditions

### 2.1 Method

- Reviewed the React surfaces, stores, desktop adapters, relevant project
  validation, and tool-window size declarations.
- Examined headless Chromium previews through the existing Vite server.
- Used `?start&nomap` for StartPage and `?nomap` for the main shell.
- Exercised normal browser-local settings and mock signal data.
- Used a mocked Tauri IPC bridge to observe startup auto-connect calls without
  opening a vehicle connection.
- Examined empty, populated, selected, and representative error/invalid-input
  states. Review screenshots are local artifacts under
  `/tmp/opencode/app-review-*.png`; regenerate durable baselines during execution.

The Motor Test files already contained ongoing working-tree changes at review
time, including `BenchHeader` and `MotorTrackingTable`. Its observations describe
that snapshot and must be rechecked against the latest implementation before
assigning corrective work.

Native WebKit/Tauri window behavior, real device enumeration, privileged helper
execution, real project-file operations, parameter writes, motor commands, and
FC transfers were not exercised by this assessment.

### 2.2 Measured observations

| Surface / scenario | Observed result |
| --- | --- |
| StartPage, 1366 × 768 | Main content width 360 px; Open/New project buttons 30 px high; no Settings entry. |
| Startup with mocked Tauri, no project | Pending endpoint existed with no countdown dialog; after the 3 s timer, one mocked `connect` call was issued. |
| Settings, 1366 × 768 | Dialog 680 × 560 px; Connection panel width 486 px. Shutdown and some Vehicle content required scrolling. |
| Connection draft | Editing the address to `10.0.0.42`, switching to Appearance, and returning restored `0.0.0.0`. |
| Custom area validation | Blank latitude/longitude still saved a named area at `(0, 0)`, while the latitude validation message remained visible. |
| Serial preset with no devices | Selector showed `No serial devices found`, with an empty DOM selection, while Connect remained enabled. |
| Tools menu | Width 208 px; six rows, each 36 px high. At 1366 px the visible “Tools” label was hidden. ArrowDown left focus on the trigger. |
| Inspector, 1366 × 768 | Columns 220 / 906 / 240 px; toolbar 40 px; a plotted chart remained 220 px high. |
| Inspector, declared minimum 800 × 480 | Fixed sidebars left 340 px for the center workspace. |
| Parameters, 1366 × 768 | Columns 190 / 946 / 230 px; toolbar about 29 px; data rows 26 px. |
| Parameters column alignment | Header width 946 px, but the first five row widths ranged from 518 to 645 px. The Value header began at x=629; first-row Value began at x=435. |
| Parameters, declared minimum 900 × 600 | Center table width 480 px; descriptions and enum meanings were heavily truncated. |
| Motor Test, 1366 × 768 | Root height 768 px, content extended to about y=1471; scope began around y=725. Lower content was clipped by the document's hidden overflow. |
| Motor Test, declared minimum 1024 × 640 | Layout switched to a vertically scrolling workspace; Stop/Emergency Stop remained in a sticky header. |
| Saved Field mode enabled | Main window applied it and enforced 44 px buttons; Inspector, Parameters, and Motor Test started without the `field-mode` class. |
| Theme changed with tool pages open | Stored theme became `light` in all pages, but only the main page applied it; the three already-open tools stayed dark. |

These are representative measurements, not a complete regression pass. The
native window minimums come from the desktop services; browser previews used
those sizes to inspect the corresponding CSS behavior.

## 3. Startup / project-selection page

### Strengths

- A clear single purpose: select a project to continue.
- Recent projects, open-folder action, and new-project creation share one store.
- Project creation exposes horizontal CRS and vertical datum explicitly.
- Project-switch refusal is displayed when the frontend detects an armed vehicle.

### Findings

| ID | Priority | Assessment | Improvement |
| --- | --- | --- | --- |
| S1 | P0 | `App` runs the telemetry bridge before returning StartPage, but auto-connect, target-change, alert, and teardown surfaces are only mounted in the main-workspace branch. The auto-connect timer is therefore active without its visible cancellation UI. | Put lifecycle/connection feedback in an application-level shell shared by startup and the workspace. Verify the no-project boot path directly. |
| S2 | P1 | StartPage has no Settings, connection status, or About entry. A user cannot adjust an endpoint, auto-connect, theme, or Field mode before opening a project. | Provide a shared Settings entry and compact current connection/configuration status on the startup shell. |
| S3 | P1 | Project import is exposed only in `ProjectHome`'s no-project branch. In the native app, that state returns StartPage instead, which has no import action. | Expose Import project on StartPage and ask for an explicit destination. Share the project-picker behavior between entry points. |
| S4 | P1 | The wizard has no visible field labels for name/CRS/datum, no destination preview, and only checks that the name is nonempty. Blank CRS leaves Create enabled; the reviewed backend copies the string without validation. | Use a labelled form with required-field validation and a visible destination. Explain horizontal CRS versus vertical height datum and validate the supported metadata contract before writing. |
| S5 | P1 | Folder-picker failures are not caught in `pickAnd`; errors from opening/creating are raw strings. Busy state mostly disables controls without identifying the active operation. | Show Opening/Creating with an operation-specific result and recovery action. Handle picker cancellation separately from errors. |
| S6 | P2 | Recents show only names and a generic dot. Paths are tooltip-only; same-name projects are hard to distinguish, and unavailable entries cannot be cleaned up. | Show a secondary path and meaningful availability/last-opened metadata when available; support locating a moved project and removing an entry from recents. |
| S7 | P2 | The fixed 360 px column leaves little room for useful context on desktop, while its fixed width and centered layout have no explicit short-window scrolling strategy. | Use an adaptive centered workspace with bounded width and vertical overflow, preserving a compact first-run state. |

### Recommended structure

```text
MagGCS                                      Settings    About
Select a project

Recent projects                         Project actions
Project name                            New project
  /path/to/project                      Open project folder…
  Availability / last opened            Import project…

                                        Connection
                                        Endpoint / current state
                                        Auto-connect countdown + Cancel

Version / support entry
```

Recent-project metadata should be factual: availability requires an actual
check; last-opened time requires persistence. Do not infer either from the
current generic status dot.

Reuse project creation/opening logic rather than maintaining separate wizard
implementations in StartPage and ProjectHome. Keep the distinction between
project metadata and application defaults explicit.

## 4. Settings assessment

### 4.1 Existing structure and strengths

Settings currently has Connection, Vehicle, Appearance, Devices, and About tabs.
It includes structured endpoint fields, connection presets, link diagnostics,
initial position and custom areas, model-orientation offsets, theme selection,
Field mode, and USB permission installation.

The stable modal size, consistent 44 px primary form controls, explicit endpoint
fields, and reuse of the teardown flow are good foundations.

### 4.2 Functional and interaction findings

| ID | Priority | Assessment | Improvement |
| --- | --- | --- | --- |
| C1 | P1 | Connection draft state belongs to the tab component. Switching tabs unmounts it and silently discards edits. | Give the Settings session ownership of drafts; preserve them across tab changes and define explicit commit/discard behavior. |
| C2 | P1 | Commit semantics vary: connection changes save on Connect, auto-connect/theme changes are immediate, coordinates commit on blur/Enter, and orientation writes on every change. There is no consistent saved/error indicator. | Document and display commit behavior. Separate desired configuration from the active connection and surface pending/saved/failed state. |
| C3 | P1 | Custom-area saving uses `Number()` on blank coordinate strings, accepting zero even though the coordinate editor rejects the blanks. | Share parsing/validation between coordinate commit and area creation; empty input must never be converted into a valid coordinate implicitly. |
| C4 | P1 | Serial mode can retain a host from another transport or an unavailable preset while the selector shows no matching device. Validation checks only a nonempty host. | Keep the selected endpoint and displayed device state consistent. Represent unavailable saved devices explicitly and validate a new serial selection before connection. |
| C5 | P1 | `config_save` failures are swallowed. A failure during config initialization also skips persistence subscription. The interface provides no indication that a preference may not survive restart. | Expose persistence status/errors and retry/recovery behavior. Cover failed load, failed save, and restart round-trip scenarios. |
| C6 | P1 | `Vehicle` mainly edits map/mock/SITL defaults and model rendering, which reads like FC configuration. | Rename and regroup it around Map & simulation / 3D model display, with clear explanation of each setting's scope. |
| C7 | P1 | Vertical tab navigation is declared horizontal to Radix. Connection labels are visually present but not associated with their controls. Theme selection has no pressed/radio semantics. | Match navigation orientation to the layout, connect labels and error descriptions to fields, and expose selected states accessibly. |
| C8 | P2 | Devices renders port/VID/PID strings with no distinct loading, scan-failed, or permission state. Permission installation acts on all enumerated USB candidates. | Show readable device identity, enumeration state, and the affected device scope before invoking the existing permission action. |
| C9 | P2 | Settings/About duplicates Tools/About, with a hardcoded version and less information. | Reuse one runtime-version/about source and one consistent information layout. |

### 4.3 Layout and typography

- **Connection:** endpoint setup, presets, connect/disconnect, auto-connect,
  diagnostics, and application shutdown all occupy one scrolling form. Separate
  primary configuration from expandable diagnostic details and application-exit
  actions. Use an unambiguous `Quit MagGCS` label for application exit.
- **Vehicle:** initial coordinates, custom areas, and orientation are successive
  sections, with orientation below the initial viewport. Use clear section
  headings and an explicit saved-value/result area.
- **Appearance:** four theme buttons are easy to scan, but Field mode enlarges
  the checkbox itself to 44 px while rem-based text becomes smaller when the
  root font changes from 16 to 14 px. Enlarge the labelled hit area and use a
  consistent text scale instead of enlarging the checkbox glyph.
- **Devices/About:** the same 680 × 560 shell contains very little content in
  empty/basic states. Preserve stable navigation, but provide useful empty-state
  guidance and a better balance between section density and whitespace.

### 4.4 Recommended settings layout

```text
Settings                                             Close

Connection          Section title
Appearance          Scope / when changes take effect
Map & simulation
Devices             Label                    Control
About               Label                    Control

                    Inline validation / connection result
                    Advanced diagnostics (expandable)

                    Saved / Pending / Save failed · Retry
```

The shared Settings surface should be reachable from StartPage, the top-bar
gear, and connection-repair entries. Each entry may choose an initial tab, but
should share the same configuration contract and feedback behavior.

## 5. Tools menu and shared window presentation

### 5.1 Menu assessment

The current six entries are Signal inspector, Motor test, Parameters, Logs,
Export diagnostics, and About MagGCS. Leading icons and aligned row labels are
useful, and the desktop services already focus existing tool windows.

The menu nevertheless mixes analysis windows, vehicle tools, an immediate
export action, and an informational dialog without grouping. Only three entries
have open/busy badges. Below the `2xl` breakpoint, the trigger is a gauge icon
without a visible Tools label, and the same gauge is used for Motor test.

The trigger has no `aria-expanded`/`aria-haspopup`; entries lack menu semantics,
and ArrowDown does not enter the menu. Tab navigation alone is not a complete
keyboard menu interaction.

### Recommended grouping

```text
Tools

Analysis & data
  Signal inspector                     Open / window indicator
  Flight logs                          Transfer state

Vehicle tools
  Motor test                           Open / running state
  Parameters                           Open / operation state

Support
  Export diagnostics                   Exporting / result
  About MagGCS
```

- Use one shared, keyboard-accessible menu with arrow navigation, Escape,
  focus restoration, and accessible open/selected state.
- Keep the Tools identity discoverable at common desktop widths.
- Separate operation state from window-open state in the trailing area.
- Use a consistent naming convention across menu, native title, and body title.
- Size the menu for its longest label plus state text, with a viewport bound.

### 5.2 Cross-window consistency findings

| ID | Priority | Finding | Improvement |
| --- | --- | --- | --- |
| T1 | P1 | Inspector, Parameters, and Motor Test read theme on boot but do not apply saved Field mode. Open tools also retain the old theme after the main window changes it. | Share preference initialization and change propagation across entry points. |
| T2 | P1 | Header heights vary from approximately 29 px in Parameters to 40 px in Inspector, 44 px in Motor Test, and 48 px in the main shell. Titles, status, and actions occupy different positions. | Define a common header/toolbar hierarchy and density scale with task-specific content slots. |
| T3 | P1 | Tool controls use independent CSS sizes; Parameters also uses clickable `div` rows that bypass global button sizing and keyboard handling. | Apply shared density rules to both controls and virtualized rows, with appropriate semantics and focus behavior. |
| T4 | P1 | Tool context is presented differently: Inspector puts its title/status at the far right, Parameters provides little connection context, and Motor Test has several distinct source/target states. | Use a consistent context area that explicitly identifies primary FC, serial target, SITL source, or replay file as appropriate. |
| T5 | P2 | Similar confirmation, status, error, and completion feedback uses different visual treatments. | Reuse status/result components and reserve colors for consistent meanings. |

Shared presentation must preserve actual connection ownership: Motor Test's
serial PX4 target and SITL source are distinct from the primary GCS link;
Inspector replay is distinct from live telemetry.

### 5.3 Main TopBar implementation update, 2026-10-23

The title, five stage tabs, tool entries, and utilities had drifted into three
visually unrelated clusters. The bar is now one 48 px row with four zones:

- **Context:** one title/project entry (`Q-Mag / <project>`), with the tool
  launchers and Settings next to it.
- **Workflow:** the five stage tabs became a wizard popover (`4/5 · Fly ▾`)
  with numbered steps, descriptions, Previous/Next, and Arrow/Home/End keys.
  Selecting a step still only changes the workspace view; numbers identify
  stages, not completion.
- **Vehicle:** status (armed/mode/link), the always-visible RTL/Hold pair, and
  health. Health still folds into *More* at ≤1366 px.
- **Utilities:** the alert badge.

High-frequency tools (Inspector, Motor test) are inline shortcuts at ≥1600 px
and fold into the Tools menu below that width. The Tools menu is a Radix
popover with arrow-key navigation and focus restoration. The map pop-out
entry was removed at the operator's request. About now has one entry point:
Settings ▸ About renders `AboutContent` with the runtime version, license,
and attribution. `.main-topbar` keeps every control at the 32 px
normal-mode floor (44 px in Field mode) via `--topbar-control-h`.

Verified with `check-topbar.mjs` at 1920 × 1080, 1280 × 800, and 1024 × 800
(status group complete and unoccluded, health folds, keyboard navigation,
responsive logos, About theme switching), `e2e-shell.mjs` (stages,
About, project entry), `check-field-mode.mjs` (44 px floor), and
`check-logs-delete.mjs` (Tools menu entry). Screenshots:
`frontend/screenshots/u2/topbar-1920.png`, `topbar-1280.png`,
`workflow-1920.png`, `tools-1920.png`.

### 5.4 Q-Mag branding update, 2026-10-23

The application display name is **Q-Mag**, including desktop product metadata,
window/browser titles, shutdown copy, and About. `AppLogo` renders the supplied
lettering in full, wordmark, and compact-Q treatments using theme tokens.
The main and standalone tool headers use Q at ≤1366 px and the wordmark above
that width. Inspector, Motor test, and Parameters put it before the tool title;
Logs and Settings use the Q at the left of their dialog title rows.

StartPage and Settings ▸ About include the full application logo and the
supplied Quaternion Aerospace company artwork. Company assets switch with the
resolved dark/light theme. StartPage is scrollable: five recent projects plus
the create form remain reachable at 1024 × 700 in Field mode
(`check-startpage.mjs`). Tool logo/title bounds were measured at minimum and
wide sizes in dark, light, and night-red; existing Inspector/Parameters/Motor
Test/Logs regressions pass with the new headers. Screenshots and manual review
steps are indexed in [frontend/README.md](../../frontend/README.md).

## 6. Layout assessment of each Tools entry

### 6.1 Signal Inspector

**Strengths:** logical signal-browser / plot-workspace / properties structure;
search, multi-signal selection, trace colors, units, and plot-grid controls.

**Implementation update, 2026-10-22 — sidebars:** the signal browser and
properties panels are now resizable (drag the divider, ←/→ to nudge,
double-click to reset) and collapsible (a header chevron folds a pane to a
reopen rail). The layout is persisted separately from the workspace
(`maggcs.inspector.layout`), so it survives a reload without *Save workspace*.
The grid uses explicit `header / notice / workspace` rows, so a notice no
longer pushes the panes into implicit rows, and each pane is clamped so the
center keeps ≥ 240 px at every window size. Verified at 800 × 480, 1366 × 768,
and 1920 × 1080 (`check-inspector.mjs`; screenshots in
`frontend/screenshots/inspector/`).

**Implementation update, 2026-10-22 — remaining 6.1 items:** a single plot now
fills the workspace height (`.plot .chart { flex: 1 }` with a 160 px floor) and
the allowed column count adapts to width (`MIN_PLOT_W = 280` → 1–3 columns;
wider presets are disabled). The tool title, a source chip (`Mock` / `Live link`
/ `No link` / `Replay · <file>`) and the rate/dropped status moved into a common
header row; the toolbar is grouped into labelled clusters (Playback, Source,
Plots, Workspace) and replay controls live in a dedicated strip that only
appears while replaying. Signal groups now expose a real disclosure button and
each trace chip an `aria-pressed` name button, so both are keyboard reachable.
Notices are severity-toned (`info` / `success` / `warn` / `error`, with
`status` / `alert` roles) and float as an overlay rather than occupying a grid
row, so a notice never displaces the workspace. `check-inspector.mjs` covers all
of the above (`frontend/screenshots/inspector/`, including
`inspector-notice-1366x768.png`).

**Improvements:**

- Make sidebars resizable/collapsible. The current 220 + 240 px fixed sidebars
  leave only 340 px at the declared 800 px minimum window width.
- Give the selected plot more usable height; a single 220 px chart leaves most
  of a large window unused.
- Adapt the allowed plot-column count to available width.
- Group toolbar controls by source, playback, plots, and workspace actions.
  Move replay controls into a dedicated strip when active, avoiding a long
  single row and repeated file/status text.
- Put the tool title and source identity in the common header position.
- Make signal groups and trace selection fully keyboard reachable.
- Distinguish success/information from errors: `.inspector-notice` currently
  uses error styling for all notices. Verify that conditional notices do not
  displace the plot workspace in the two-row grid.

### 6.2 Parameters

**Strengths:** metadata-first browsing, offline descriptions, search, grouped
navigation, a detail editor, and a staged-change tray.

**Implementation update, 2026-10-22 — geometry and columns:** the defect had a
single cause: a virtualized `.p-row` was absolutely positioned with only `top`
set, so it shrink-to-fit its content and resolved its own `fr`/`minmax` widths
differently from the full-width header (reproduced: the Value column origin was
193.6 px off). Rows now pin `left: 0; right: 0`, the header and rows share one
`--p-param-cols` template, and the header's right padding matches the scroll
gutter so both content boxes are identical. Value and Default were rebalanced
from fixed 90/70 px to flexible shares, and their full text is exposed via a
title (the detail pane already shows it on selection). Verified by
`check-params.mjs` at 1366 × 768, 1920 × 1080, and 900 × 600, after a search, a
group switch, and deep scrolling (screenshots in `frontend/screenshots/params/`).

**Implementation update, 2026-10-22 — remaining 6.2 items:** the group pane and
editor are now on their own grid tracks with draggable splitters (pointer and
ArrowLeft/Right, double-click reset) and collapse to rails, persisted under
`maggcs.params.layout`, so the table keeps `MIN_CENTER_W` (460 px) at every size.
The two `Custom` controls are disambiguated: the pane tab reads **My groups**
(the operator's sets) and the toolbar filter reads **In groups** (membership
only). The list is a focusable `role="listbox"`: ArrowUp/Down, PageUp/Down and
Home/End move the selection and scroll it into view, with `aria-activedescendant`
tracking it; the virtualizer reads the `--p-row-h` variable and observes the
`field-mode` class, so Field mode enlarges rows to the 44 px touch floor and the
window now mirrors Field mode at boot. Current FC identity/status is a chip next
to the title (`Mock` / `No FC` / `PX4 <ver>`, warning-toned on a metadata
mismatch) replacing the standalone version strip, and the staged-change tray is
bounded (`max-height: 60px`) and scrollable so every pending entry and the Apply
button stay reachable. `check-params.mjs` covers all of the above
(`params-collapsed.png`, `params-staged-tray.png`).

**Implementation update, 2026-10-23 — current/default presentation:** the
right-aligned numeric cells sat under left-aligned **Value**/**Default**
headers, so a current value visually appeared in the Default column. Both
headers now share the values' right edge. The detail pane shows **Current
value** and **Default** before the firmware description (previously a long
description pushed the default below the fold), both wrap in full, and each new
selection scrolls the pane back to that summary. `check-params.mjs` now also
asserts column widths and actual value-text alignment, and no horizontal
overflow at the widest pane size; `check-params-connection.mjs` covers long enum
meanings at 900 × 600 and with a 140 px editor. The table's minimum content
width is 474 px plus the live scrollbar gutter.

**Original confirmed layout defect (fixed in the updates above):** virtualized
`.p-row` elements were absolutely positioned without a common full-width
constraint. Their automatic widths varied with content, so each row computed
different fractional columns even though it used the same grid template as the
header. Numeric values consequently drifted between rows and no longer sat
under the Value/Default headers.

**Improvements:**

- First fix row width and shared column geometry. Header/body column origins
  must align within 1 CSS px at all supported sizes and scroll positions.
- Rebalance value/default widths: 90/70 px truncates enum descriptions even on
  wider windows. Make full values reachable without relying only on hover.
- Allow the 190 px group pane and 230 px editor to resize/collapse at narrow
  widths.
- Clarify the two separate `Custom` controls: one chooses a group pane; the
  other filters membership.
- Add keyboard row navigation and selection, and keep virtualized row height
  synchronized with the selected density. `ROW_H = 26` must not remain a
  separate assumption when Field mode enlarges rows.
- Put current FC identity/status near the title, and keep the staged-change
  tray bounded and reachable with many pending entries.

### 6.3 Motor Test / Motor Bench

**Implementation update, 2026-10-22:** the Motor Test-only follow-up is complete.
Unlock/start and Mag alarm now occupy the left sidebar; viewport height and
stacked scrolling are bounded; inactive input and secondary logging details
collapse; the header has one test-state label. Motor Test also applies and
synchronizes theme/Field mode. See the [module notes, checks, and screenshots](../../frontend/src/actuator/README.md).
The snapshot findings below retain the assessment that motivated this work.

**Strengths in the reviewed working tree:** explicit target identity and stop
controls in `BenchHeader`; a useful per-motor comparison table; sticky
Stop/Emergency Stop on narrow layouts; separation of input and serial target.

**Confirmed layout defect in that snapshot:** at 1366 × 768, the grid content
grew to about 1471 px while the document suppressed outer scrolling. The scope
started around the bottom edge of the viewport, leaving lower plots and logging
content inaccessible through the intended layout.

**Improvements:**

- Establish a bounded workspace row with explicit minimum sizes and scrolling
  ownership. Ensure the center plot and right-side logging controls remain
  reachable at 1366 × 768 as well as the declared minimum size.
- Keep Stop and Emergency Stop visible while the workspace scrolls.
- Allocate usable height to plots and avoid simultaneous nested percentage
  caps that depend on an unconstrained parent height.
- Collapse inactive source configuration and secondary logging details.
- Reconcile duplicated state labels: the header currently uses `IDLE` while
  the center can say `LOCKED`; show one primary state with distinct secondary
  prerequisite information.
- Standardize the menu/native/body names, currently Motor test, Motor / Mag
  Interference Test, and Motor Bench.
- Rebase all follow-up work on the existing in-progress Motor Test changes.

### 6.4 Logs

Use the [separate Logs assessment](logs-ui-review-plan.md) for detailed findings
and implementation acceptance. Its layout should participate in the same
header, typography, status, and Field-mode rules established here.

The main recommendation remains Flight logs / MAVLink console tabs, a compact
log table, and a visible transfer summary, after cancellation/disconnect fixes.

### 6.5 Export diagnostics

This entry performs an immediate export rather than opening a workspace. The
menu closes immediately; completion/failure is sent through the alert system.
A successful export is an Info alert, so its path is not presented as a direct
result surface like a warning toast or critical banner.

**Recommendation:** show an exporting state, then a concise completion result
with Open folder and Copy path. Keep errors actionable and retain the result
long enough to use it. Avoid duplicate exports while the first is active.

### 6.6 About

Tools/About is a 320 px-wide dialog positioned near the top, while
Settings/About is a sparse tab in a much larger centered dialog. They also use
different version sources.

**Recommendation:** one shared About content source, consistent title/close
controls, a readable bounded width, runtime version/build information, and the
existing license/data-attribution content. Both entry points should agree.

**Delivered 2026-10-23:** About is now exclusively in Settings and uses
`AboutContent` for the runtime version, Q-Mag/company branding, license, and
NRCan attribution. The duplicate Tools/About dialog was removed.

## 7. Proposed shared visual rules

Confirm these as design contracts before applying them:

| Area | Proposed rule |
| --- | --- |
| Typography | Tool title 16–18 px; section heading 13–14 px; primary data/body 12–14 px. Essential content should not depend on 10 px text. |
| Numeric data | Monospaced tabular numerals with units in headers or consistently beside values; right-align comparable numeric columns. |
| Spacing | Reuse 4/8/12/16/24 px tokens; use consistent heading-to-content and section-to-section spacing. |
| Controls | Normal toolbar target at least 32 px; Field mode target at least 44 × 44 px, including selectable rows and their virtualization geometry. |
| Header | Title and source/target context first, task actions next, status and persistent stop controls in predictable positions. |
| Workspace | Each pane has an explicit size boundary and scrolling owner; long content must remain reachable without obscuring persistent actions. |
| Tables | Shared header/body column geometry; full values accessible; visible selected/focused state. |
| Feedback | Pending, success, warning, and error have consistent text/icon/color treatment; essential results are not hidden in a count badge. |
| Accessibility | Correct menu/tab orientation and semantics, labelled fields, keyboard navigation, focus restoration, and non-color state cues. |
| Preferences | Theme and Field mode apply before first paint and propagate to already-open windows. |

## 8. Deferred execution plan

**WP3a, WP3b, and WP8 are complete**, together with the Motor Test and Parameters
portions of WP4. WP7's menu/About work is delivered; diagnostics feedback
remains open. Split remaining packages into smaller reviewed patches.

| WP | Priority | Work | Acceptance |
| --- | --- | --- | --- |
| 0 | P1 | Confirm shared shell, Settings commit semantics, grouping/naming, and the size/density contracts. Present affected interfaces and error types. | Approved wireframes and state tables cover startup/main/tool windows, success, failure, unavailable devices, and pending operations. |
| 1 | P0 | Move startup lifecycle feedback and Settings access into the shared application shell. | With no project, the endpoint/countdown is visible before auto-connect; Cancel results in zero connection calls; target/teardown feedback and critical alerts are reachable. Settings opens before project selection. |
| 2a | P1 | Fix Settings draft retention, coordinate parsing, and serial selection consistency as separate small tasks. | Tab changes preserve drafts; blank/invalid coordinates create no area; the displayed serial selection matches the endpoint that would be used. |
| 2b | P1 | Add configuration persistence feedback and recovery. | Simulated load/save failures are visible; a successful retry survives restart; failed initialization does not silently disable all future persistence. |
| 3a | P1 | **Completed 2026-10-22:** Parameters table geometry fixed (rows pinned full width, shared `--p-param-cols`, scroll-gutter-matched header) and Value/Default rebalanced. | Each data-column origin differs from its header by at most 1 CSS px at 1366, 1920, and 900 px widths, including after search, group switching, and deep scrolling (`check-params.mjs`). |
| 3b | P1 | **Completed 2026-10-22:** Motor Test sidebar start/alarm controls, bounded workspace, stacked order, consistent state/title, and optional input/log details. | Simulated IPC regression passes at 1024 × 640, 1366 × 768, and 1920 × 1080: plots/capture reachable, stop controls visible, no wide document overflow. Build and app compilation pass; screenshots and manual steps are in the module README. |
| 4 | P1 | Share theme/Field-mode initialization and live propagation; update density-dependent controls and virtualized rows. | Main, Inspector, Parameters, and Motor Test agree on saved preferences at startup and update within 500 ms after a change; Field-mode targets meet 44 × 44 px without overlap. |
| 5 | P1/P2 | **Partly completed 2026-10-22:** no-project entry, shared name-only create wizard, and recent remove/delete. Remaining: import on StartPage and recents availability/last-opened metadata. | Create/open work from StartPage; delete validates `project.json` before removing and never deletes an open project. Import from a fresh install and clear recovery for missing/locked/corrupt projects remain open. |
| 6 | P1/P2 | Regroup and restyle Settings, including vertical navigation, accessible fields, clearer setting scope, and consistent saved state. | Keyboard-only navigation reaches every control; Connection and Map/simulation sections remain usable at target sizes; setting scope and commit behavior are visible. |
| 7 | P1/P2 | **Partly completed 2026-10-23:** compact Q-Mag TopBar, wizard stage navigation, inline high-frequency shortcuts, keyboard-accessible Tools menu, removed map pop-out entry, and About merged into Settings (`AboutContent`). Remaining: diagnostics busy/success/failure feedback. | Arrow keys/Enter/Escape work; focus returns to the trigger; window/task states are distinguishable; diagnostics completion exposes its result; About versions/content agree. |
| 8 | P2 | **Completed 2026-10-22:** Inspector sidebars resizable/collapsible, plot fills height with width-capped columns, common header (title/source/status), grouped toolbar with a replay strip, keyboard-reachable signal groups and trace selection, and severity-toned overlay notices. Logs handled by its own plan. | At 800 × 480 and both desktop sizes, primary controls and selected plots remain usable; long replay names and notices do not break the workspace (`check-inspector.mjs`). |

If a follow-up task changes privileged USB installation, serial output, or
device configuration, perform the repository-required failure/wrong-device
analysis before implementing that task. Layout checks should use mocked
backends for those actions.

## 9. Verification and manual review matrix

### Automated or measurable checks

- [x] No-project startup: lifecycle overlays now mount on StartPage, so the
  auto-connect countdown, cancellation, target confirmation, and alerts are
  present; the no-project workspace is entered through *Continue without a
  project* (`check-startpage.mjs`).
- [ ] Settings: draft retention, invalid/blank coordinate rejection, unavailable
  serial selection, and persistence failure/retry.
- [x] Project creation: name-only contract with a default or operator-chosen
  destination; recent removal and typed-name disk deletion. Import from a fresh
  install is still open.
- [x] Parameter table geometry: header/body column origins within 1 px at
  1366 × 768, 1920 × 1080, and 900 × 600, after search, group switch, and deep
  scroll; Value/Default rebalanced (`check-params.mjs`).
- [x] Parameter table: last-row reachability (keyboard End) and Field-mode
  row-height consistency (virtual span = rows × row height); resizable/
  collapsible panes; distinct pane/filter labels; FC status chip; bounded
  staged tray (`check-params.mjs`; `params-collapsed.png`, `params-staged-tray.png`).
- [x] Motor Test: all workspace content reachable; persistent stop controls at
  supported sizes; verified against the integrated working-tree UI on 2026-10-22.
- [x] Inspector sidebars: resizable/collapsible, layout persisted, and a
  ≥ 240 px workspace at 800 × 480, 1366 × 768, and 1920 × 1080
  (`check-inspector.mjs`; screenshots in `frontend/screenshots/inspector/`).
- [x] Inspector workspace: one plot fills the height, columns cap at the
  available width (3 at 1920, 1 at 800), the header carries title/source/status,
  the toolbar is grouped with a dedicated replay strip, signal groups and trace
  selection are keyboard reachable, and notices are severity-toned overlays that
  do not displace the workspace (`check-inspector.mjs`;
  `inspector-notice-1366x768.png`).
- [ ] Shared preference boot and live synchronization across all tool windows.
- [x] Main TopBar: single-row context/workflow/vehicle/utilities zones, wizard
  stage navigation, wide-screen tool shortcuts, keyboard-accessible Tools menu, and
  About merged into Settings; status group stays complete and unoccluded and
  every control clears the 32 px floor at 1920 × 1080 and 1280 × 800
  (`check-topbar.mjs`, `e2e-shell.mjs`, `check-field-mode.mjs`,
  `check-logs-delete.mjs`; screenshots in `frontend/screenshots/u2/`).
- [x] Tools keyboard behavior and focus restoration; expanded states available
  to assistive technology (`check-topbar.mjs`).
- [x] Q-Mag branding in main/tool headers, startup and About; company assets
  switch with theme. Five-recents startup form stays reachable in Field mode.
- [x] About is unified in Settings, with runtime version and complete credits.
- [ ] Diagnostics busy/success/failure feedback.

The existing `check-startup.mjs` injects an open mock project before checking
startup dialogs. `check-startpage.mjs` now provides the genuine **no-project**
fixture: it previews StartPage with `?start`, exercises the name-only wizard,
recent remove/delete, and entering the workspace with *Continue without a
project*, using an isolated IPC stub (no files touched).

### Screenshot and manual scenarios

| Matrix | Required views |
| --- | --- |
| Desktop | 1366 × 768 and 1920 × 1080. |
| Declared tool minimums | Inspector 800 × 480; Parameters 900 × 600; Motor Test 1024 × 640. |
| Appearance | Dark, light, night-red; Field mode on/off. |
| Startup | No recents, five recents including long/same names, new-project form, operation error, auto-connect pending. |
| Settings | Each tab, dirty draft, inline error, unavailable device, long device list, save failure. |
| Tools | Menu with open/busy states; Inspector empty/populated/replay; Parameters selected and staged; Motor Test dense content; Logs states from its plan; diagnostics result; About. |
| Input | Mouse and keyboard-only completion; appropriate touch targets in Field mode. |

Native follow-up should verify file pickers, real window sizing/focus, device
enumeration, and persisted settings after restart. Backend-affecting tasks need
the applicable unit/SITL checks in addition to screenshots.

## 10. Source map and resumption notes

| Area | Main files |
| --- | --- |
| Application/startup | [App.tsx](../../frontend/src/App.tsx), [StartPage.tsx](../../frontend/src/components/StartPage.tsx), [bridge.ts](../../frontend/src/desktop/bridge.ts), [autoconnect.ts](../../frontend/src/stores/autoconnect.ts) |
| Project entry | [ProjectHome.tsx](../../frontend/src/components/dialogs/ProjectHome.tsx), [project store](../../frontend/src/stores/project.ts), [project service](../../crates/app-tauri/src/project_service.rs), [project model](../../crates/core/src/project/model.rs) |
| Settings/configuration | [SettingsDialog.tsx](../../frontend/src/components/dialogs/SettingsDialog.tsx), [endpoint.ts](../../frontend/src/desktop/endpoint.ts), [prefs.ts](../../frontend/src/desktop/prefs.ts), [appConfig.ts](../../frontend/src/desktop/appConfig.ts), [presets.ts](../../frontend/src/cesium/presets.ts) |
| Menu / About / alerts | [TopBar.tsx](../../frontend/src/components/TopBar.tsx), [WorkflowNav.tsx](../../frontend/src/components/WorkflowNav.tsx), [topbar.css](../../frontend/src/components/topbar.css), [AboutContent.tsx](../../frontend/src/components/dialogs/AboutContent.tsx), [AlertLayer.tsx](../../frontend/src/components/AlertLayer.tsx) |
| Shared design | [components.tsx](../../frontend/src/design-system/components.tsx), [index.css](../../frontend/src/design-system/index.css), [theme.ts](../../frontend/src/design-system/theme.ts), [ui.ts](../../frontend/src/stores/ui.ts) |
| Branding | [AppLogo.tsx](../../frontend/src/components/AppLogo.tsx), [CompanyLogo.tsx](../../frontend/src/components/CompanyLogo.tsx), [branding.css](../../frontend/src/components/branding.css), [assets](../../frontend/public/brand/) |
| Inspector | [InspectorApp.tsx](../../frontend/src/inspector/InspectorApp.tsx), [inspector.css](../../frontend/src/inspector/inspector.css), [entry](../../frontend/src/inspector/main.tsx) |
| Parameters | [ParamsApp.tsx](../../frontend/src/params/ParamsApp.tsx), [params.css](../../frontend/src/params/params.css), [entry](../../frontend/src/params/main.tsx) |
| Motor Test | [ActuatorApp.tsx](../../frontend/src/actuator/ActuatorApp.tsx), [actuator.css](../../frontend/src/actuator/actuator.css), [BenchHeader.tsx](../../frontend/src/actuator/BenchHeader.tsx), [MotorTrackingTable.tsx](../../frontend/src/actuator/MotorTrackingTable.tsx), [entry](../../frontend/src/actuator/main.tsx) |

On resumption, recheck the working tree, refresh screenshots for changed
surfaces, confirm WP0, and execute the highest-priority small task. Coordinate
Logs work with its existing plan and avoid duplicating ongoing Motor Test work.
