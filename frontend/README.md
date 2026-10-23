# Q-Mag React frontend

React/TypeScript, Cesium, Radix UI, shared design tokens, and English i18n.
See [ADR-010](../docs/adr/010-frontend-stack.md).

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
