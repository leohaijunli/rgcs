// End-to-end check for the graded command interaction (plan §7 U4, acceptance:
// "a stray single click must not send a medium- or high-risk command; RTL is
// at most two interactions from any stage").
//   node check-command-ux.mjs <viteUrl> <shotsDir>
//
// The fake FC lives in the page: it streams telemetry, answers
// `flight_mode_table`, Runtime client ownership, and every typed command the UI
// hands to `flight_command_execute`. Deferred operation-bound replies let the
// check observe the awaiting-ack phase and stale-target rejection.
//
// High risk (force-arm L5, quit/disconnect L4) is the keyword tier and lives in
// `check-teardown.mjs`; this file covers the click and hold tiers.
import { createRequire } from 'node:module'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const require = createRequire(import.meta.url)
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright')

const [url, shotsDir] = [process.argv[2], process.argv[3]]
const flightShotsDir = process.env.FLIGHT_TARGET_SHOTS_DIR ?? shotsDir
const mapMode = process.argv.includes('--map')
if (!url || !shotsDir) {
  console.error('usage: node check-command-ux.mjs <viteUrl> <shotsDir> [--map]')
  process.exit(1)
}

/** Mirrors HOLD_MS in frontend/src/design-system/components.tsx. */
const HOLD_MS = 1000
/** Deliberately longer than HOLD_MS so the hold is a hold, not a race. */
const HOLD_AT_MS = 1600

let failed = 0
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`)
  if (!ok) failed += 1
}

// Headless Chromium renders the Cesium map on a software GL stack, which
// starves the page's own timers, so every wait is driven from Node by polling
// the DOM (`waitUntil`) instead of trusting a page-side timeout.
let page = null
async function waitUntil(fn, timeoutMs, stepMs = 100) {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    if (await fn()) return true
    if (Date.now() >= deadline) return false
    await page.waitForTimeout(stepMs)
  }
}

const browser = await chromium.launch({ headless: true, args: ['--disable-gpu', '--disable-dev-shm-usage'] })
page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
const pageErrors = []
page.on('pageerror', (e) => {
  pageErrors.push(String(e))
  console.log(`  page error: ${e}`)
})
page.on('console', (m) => {
  if (m.type() === 'error') pageErrors.push(m.text())
})

await page.addInitScript(() => {
  const handlers = {}
  let fired = 0

  const deliver = (event, payload) => {
    const handler = handlers[event]
    if (handler === undefined) return false
    const cb = window[`__mock_cb_${handler}`]
    if (typeof cb !== 'function') return false
    fired += 1
    cb({ event, id: fired, payload })
    return true
  }

  // PX4 custom_mode packing, same as the UI's (ADR-017).
  const pack = (main, sub) => ((main & 0xff) << 16) | ((sub & 0xff) << 24)

  let armed = true
  let customMode = pack(4, 3) // Auto · Loiter
  let modeLabel = 'Hold'
  let feedTimer = undefined

  const snapshot = (atMs) => ({
    last_heartbeat_at_ms: atMs % 100000,
    heartbeat: {
      system_id: targetRevision,
      component_id: 1,
      vehicle_type: 'quadrotor',
      autopilot: 'px4',
      base_mode: {
        custom_mode_enabled: true,
        test_enabled: false,
        auto_enabled: true,
        guided_enabled: true,
        stabilize_enabled: true,
        hil_enabled: false,
        manual_input_enabled: false,
        safety_armed: armed,
      },
      custom_mode: customMode,
      mode_label: modeLabel,
      flight_state: 'active',
      mavlink_version: 3,
    },
    global_position: {
      time_boot_ms: 1,
      latitude_deg: 49.2831,
      longitude_deg: -123.1207,
      altitude: { datum: 'AMSL_EGM96', meters: 48.5 },
      relative_alt_m: 40,
      velocity: { x_m_s: 0, y_m_s: 0, z_m_s: 0 },
      heading_deg: 12,
    },
    attitude: { roll_deg: 0.3, pitch_deg: -0.4, yaw_deg: 12 },
    sys_status: {
      sensors: {},
      battery_voltage_mv: 15400,
      battery_current_ma: 12000,
      battery_remaining_percent: 76,
    },
    battery: null,
    gps: { fix_type: 'RTK_FIXED', satellites_visible: 18 },
    field_ages: {
      heartbeat_at_ms: atMs,
      global_position_at_ms: atMs,
      attitude_at_ms: atMs,
      sys_status_at_ms: atMs,
      battery_at_ms: null,
      gps_at_ms: atMs,
    },
  })

  window.__mockCommands = []
  window.__mockUnhandled = []
  let targetRevision = 1
  const runtimeSessions = new Map()
  let operation = 0
  const pendingFlights = []
  const pendingChecklists = []
  window.__mockDeferChecklist = false
  window.__mockResolveChecklist = () => {
    const resolve = pendingChecklists.shift()
    if (!resolve) throw new Error('no pending checklist')
    resolve({ items: [
      { id: 'link_fresh', group: 'link', status: 'pass', detail: 'Current link', fix_hint: null },
      { id: 'gps_fix', group: 'navigation', status: 'fail', detail: 'Obsolete target GPS refusal', fix_hint: null },
    ] })
  }
  window.__mockPendingChecklistCount = () => pendingChecklists.length
  const runtimeTarget = () => ({
    vehicle_id: `vehicle-${targetRevision}`,
    session_id: `target-session-${targetRevision}`,
    link_id: `link-${targetRevision}`,
    generation: `${targetRevision}`,
    mavlink_target: { system_id: targetRevision, component_id: 1 },
  })
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b)
  const flightReply = (entry, result, target = entry.request.context.session.target) => ({
    operation: {
      id: `operation-${++operation}`,
      owner: structuredClone(entry.request.context.session.owner),
      target: structuredClone(target),
    },
    command: entry.request.intent.kind,
    steps: [{ step: entry.request.intent.kind, result }],
  })
  window.__mockFlightReply = (kind, result = 'accepted', target = null) => {
    const index = pendingFlights.findIndex((entry) => entry.request.intent.kind === kind)
    if (index < 0) throw new Error(`no pending ${kind} flight command`)
    const [entry] = pendingFlights.splice(index, 1)
    entry.resolve(flightReply(entry, result, target ?? entry.request.context.session.target))
  }
  window.__mockFlightError = (kind, error) => {
    const index = pendingFlights.findIndex((entry) => entry.request.intent.kind === kind)
    if (index < 0) throw new Error(`no pending ${kind} flight command`)
    const [entry] = pendingFlights.splice(index, 1)
    entry.reject(error)
  }
  window.__mockSetRuntimeTarget = (revision) => { targetRevision = revision }
  window.__mockFeedStart = (intervalMs = 250) => {
    clearInterval(feedTimer)
    window.__mockTelemetry(Date.now())
    feedTimer = setInterval(() => window.__mockTelemetry(Date.now()), intervalMs)
  }
  window.__mockFeedStop = () => clearInterval(feedTimer)
  window.__mockTelemetry = (atMs) => deliver('telemetry', snapshot(atMs))
  window.__mockLink = (status) => deliver('link', status)
  // Readback for a mode switch: the heartbeat is the truth (ADR-017).
  window.__mockMode = (main, sub, label) => {
    customMode = pack(main, sub)
    modeLabel = label
    window.__mockTelemetry(Date.now())
  }
  window.__mockArm = (next) => {
    armed = next
    window.__mockTelemetry(Date.now())
  }
  const modeEntry = (id, label, main, sub, confirm) => ({
    option: { id, label, main_mode: main, sub_mode: sub, confirm, advanced: false },
    enabled: true,
    reason: null,
  })

  window.__TAURI_INTERNALS__ = {
    transformCallback: (cb, once = false) => {
      const id = Math.floor(Math.random() * 1e9)
      window[`__mock_cb_${id}`] = (result) => {
        cb(result)
        if (once) delete window[`__mock_cb_${id}`]
      }
      return id
    },
    invoke: async (cmd, args) => {
      if (cmd === 'plugin:event|listen') {
        handlers[args.event] = args.handler
        return fired + 1000
      }
      if (cmd.startsWith('plugin:event|')) return null
      if (cmd === 'alert_snapshot') return []
      if (cmd === 'enumerate_devices') return []
      if (cmd === 'link_status' || cmd === 'connect' || cmd === 'link_error') return null
      if (cmd === 'runtime_targets') {
        const target = runtimeTarget()
        return [{
          link_id: target.link_id,
          target,
          availability: 'available',
          heartbeat_age_ms: 0,
          parameter_service_available: true,
          roles: ['primary'],
          transport_owner: 'primary',
          endpoint: 'mock-primary',
        }]
      }
      if (cmd === 'runtime_client_attach') {
        const runtimeSession = {
          owner: { client_id: 'command-ux-client', instance_id: `${args.moduleId}-${targetRevision}` },
          target: structuredClone(args.target),
          module_id: args.moduleId,
        }
        runtimeSessions.set(args.moduleId, runtimeSession)
        return structuredClone(runtimeSession)
      }
      if (cmd === 'runtime_client_validate') {
        if (!same(runtimeSessions.get(args.session.module_id), args.session)) throw { code: 'not_owner' }
        if (!same(args.session.target, runtimeTarget())) throw { code: 'runtime', error: { code: 'target_changed' } }
        return null
      }
      if (cmd === 'runtime_client_detach') {
        if (same(runtimeSessions.get(args.session.module_id), args.session)) runtimeSessions.delete(args.session.module_id)
        return null
      }
      if (cmd === 'flight_command_execute') {
        window.__mockCommands.push({ cmd, ...(args ?? {}) })
        return new Promise((resolve, reject) => pendingFlights.push({ request: structuredClone(args.request), resolve, reject }))
      }
      if (cmd === 'preflight_checklist') {
        if (window.__mockDeferChecklist) return new Promise((resolve) => pendingChecklists.push(resolve))
        return { items: [{ id: 'link_fresh', group: 'link', status: 'pass', detail: 'Current link', fix_hint: null }] }
      }
      if (cmd === 'flight_mode_table') {
        return [
          modeEntry('hold', 'Hold', 4, 3, false),
          modeEntry('mission', 'Mission', 4, 4, false),
          modeEntry('return', 'Return', 4, 5, true),
          modeEntry('land', 'Land', 4, 6, true),
        ]
      }
      window.__mockCommands.push({ cmd, ...(args ?? {}) })
      window.__mockUnhandled.push(cmd)
      return null
    },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
})

await page.goto(
  mapMode ? url : `${url}` + (url.includes('?') ? '&' : '?') + 'nomap',
  { waitUntil: 'networkidle', timeout: 60000 },
)
await page.evaluate(() => {
  // The forced start page (PJ3) gates the shell behind Tauri; the mock opens
  // a throwaway project so the surfaces under test render.
  window.__mgProject?.setState({ project: { version: 1, name: 'Mock', crs: 'EPSG:4326', datum: 'cgvd2013', areas: [], settings: null } })
})
await page.evaluate(() => {
  window.__mockFeedStart()
  window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: true })
})
// L8: the startup dialogs gate the shell — let the auto-connect window
// expire and confirm the (first) target change before anything clicks.
await page.waitForSelector('[data-testid="autoconnect-dialog"]', { state: 'detached', timeout: 15000 }).catch(() => {})
const targetConfirm = page.locator('[data-testid="target-change-confirm"]')
await targetConfirm.waitFor({ state: 'visible', timeout: 8000 }).catch(() => {})
if ((await targetConfirm.count()) > 0) await targetConfirm.click()
await page.waitForSelector('[data-testid="target-change-dialog"]', { state: 'detached', timeout: 5000 }).catch(() => {})

await mkdir(shotsDir, { recursive: true })
await mkdir(flightShotsDir, { recursive: true })
await waitUntil(async () => (await page.locator('[data-testid="flight-menu"]').count()) === 1, 20000)

// Optional real-map acceptance: use Cesium's actual RIGHT_CLICK handler to
// open the production context menu. Default CI remains `?nomap` because some
// headless SwiftShader runners cannot keep a Cesium scene alive (A8).
if (mapMode) {
  const canvas = page.locator('.cesium-widget canvas').first()
  await canvas.waitFor({ state: 'visible', timeout: 30000 })
  const box = await canvas.boundingBox()
  if (box === null) throw new Error('Cesium canvas has no layout box')
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2, { button: 'right' })
  const mapMenu = page.locator('[data-testid="map-context-menu"]')
  check(await waitUntil(async () => (await mapMenu.count()) === 1, 10000),
    'a real Cesium right-click opens the map context menu')
  check(await page.locator('[data-testid="map-change-altitude"]').isVisible(),
    'the right-click menu exposes Change altitude')
  check(await page.locator('[data-testid="map-change-speed"]').isVisible(),
    'the right-click menu exposes Change speed')
  await page.screenshot({ path: join(shotsDir, 'flight-target-map-context-menu.png') })
  await page.locator('[data-testid="map-change-speed"]').click()
  check(await page.locator('[data-testid="flight-target-dialog"]').isVisible(),
    'Change speed opens the flight-target dialog from the right-click menu')
  await page.screenshot({ path: join(shotsDir, 'flight-target-speed-dialog.png') })
  await page.locator('[data-testid="flight-target-cancel"]').click()

  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2, { button: 'right' })
  await page.locator('[data-testid="map-change-altitude"]').click()
  check(await page.locator('[data-testid="flight-target-dialog"]').isVisible(),
    'Change altitude opens the flight-target dialog from the right-click menu')
  await page.screenshot({ path: join(shotsDir, 'flight-target-altitude-dialog.png') })
  await page.locator('[data-testid="flight-target-cancel"]').click()
}

/** Every command the UI handed to the backend (boot-time invokes excluded). */
const commands = () =>
  page.evaluate(() =>
    window.__mockCommands.filter((c) => c.cmd === 'flight_command_execute'),
  )
const status = page.locator('[data-testid="command-status"]')

/** Press and hold a button for `ms` (with an optional mid-hold snapshot). */
async function hold(locator, ms, midShot = null) {
  await locator.hover()
  await page.mouse.down()
  if (midShot) {
    await page.waitForTimeout(450)
    await midShot()
  }
  await page.waitForTimeout(ms)
  await page.mouse.up()
}

// --- 0. idle: no command, no strip ------------------------------------------
check((await commands()).length === 0, 'nothing is sent before the operator acts')
check((await status.count()) === 0, 'the status strip is empty until a command exists')

// --- 1. the menu carries the state a command will run against ---------------
await page.locator('[data-testid="flight-menu"]').click()
const bar = page.locator('[data-testid="confirm-bar"]')
check((await bar.count()) === 1, 'the confirm bar appears above the flight commands')
const barMode = await page.locator('[data-testid="confirm-mode"]').innerText()
const barAgl = await page.locator('[data-testid="confirm-agl"]').innerText()
const barBatt = await page.locator('[data-testid="confirm-battery"]').innerText()
check(barMode.includes('MODE') && barMode.includes('Hold'), `the bar names the mode (got "${barMode}")`)
// 48.5 m AMSL, 40 m above HOME; no DEM is loaded in this run, so the HOME
// plane is the AGL reference (cesium/heights.ts) and the height is 40.0 m.
check(/AGL\s+40\.0 m/.test(barAgl), `the bar names the height above ground (got "${barAgl}")`)
check(barBatt.includes('76%'), `the bar names the battery (got "${barBatt}")`)
await page.screenshot({ path: join(shotsDir, 'command-menu.png') })

// --- 2. a single click cannot send any flight command (mis-tap safety) ------
for (const key of ['takeoff', 'land', 'rtl', 'arm_disarm']) {
  await page.locator(`[data-testid="flight-cmd-${key}"]`).click()
  await page.waitForTimeout(150)
}
check(
  (await commands()).length === 0,
  'one click on each of Takeoff / Land / RTL / Arm sends nothing',
)
check(
  (await status.count()) === 0,
  'no command status strip appears for a click that sent nothing',
)

// --- 3. a hold short of the window still sends nothing ----------------------
await hold(page.locator('[data-testid="flight-cmd-land"]'), 350)
await page.waitForTimeout(200)
check((await commands()).length === 0, 'a hold released before the window sends nothing')

// --- 4. the full hold sends, with the action shown while it runs -----------
let midHold = null
await hold(page.locator('[data-testid="flight-cmd-land"]'), HOLD_AT_MS, async () => {
  midHold = await page.locator('[data-testid="flight-cmd-land"]').innerText()
  const dash = await page
    .locator('[data-testid="flight-cmd-land"] rect')
    .getAttribute('stroke-dasharray')
  check(
    (await page.locator('[data-testid="flight-cmd-land"]').getAttribute('data-holding')) === 'true',
    'the button reports the hold in progress',
  )
  check(
    Number.parseFloat(dash ?? '0') > 0,
    `the progress ring fills while holding (dasharray ${dash})`,
  )
  await page.screenshot({ path: join(shotsDir, 'command-holding.png') })
})
check(
  midHold !== null && midHold.includes('Land on the ground below'),
  `the hold names what it will execute (got "${midHold}")`,
)
const afterHold = await commands()
check(
  afterHold.some((c) => c.request.intent.kind === 'land'),
  'the completed hold sends the land command',
)
const landRequest = afterHold.find((c) => c.request.intent.kind === 'land')
check(
  landRequest?.request.intent.ground_amsl?.datum === 'AMSL_EGM96',
  'the landing terrain height is datum-tagged',
)
check(afterHold.length === 1, `exactly one command goes out (got ${afterHold.length})`)

// --- 5. the status strip: sent -> awaiting ack -> result + elapsed ----------
check(await waitUntil(async () => (await status.count()) === 1, 5000), 'the status strip appears')
const sentText = await status.innerText()
check(sentText.includes('Land'), `the strip names the command (got "${sentText}")`)
check(
  sentText.includes('Sent — awaiting ack'),
  `the strip shows the awaiting-ack phase (got "${sentText}")`,
)
const sentAt = Date.now()
await page.waitForTimeout(900)
await page.evaluate(() => window.__mockFlightReply('land'))
check(
  await waitUntil(async () => (await status.innerText()).includes('Accepted'), 5000),
  'the strip shows the terminal ack',
)
const elapsedText = await page.locator('[data-testid="command-status-elapsed"]').innerText()
const elapsedMs = Number.parseFloat(elapsedText) * 1000
check(
  Number.isFinite(elapsedMs) && Date.now() - sentAt - elapsedMs < 1500,
  `the strip reports the round-trip time (got "${elapsedText}")`,
)
check(
  (await status.getAttribute('data-command')) === 'Land',
  'the strip keeps the command name after the ack',
)
await page.screenshot({ path: join(shotsDir, 'command-result.png') })

// --- 6. RTL is at most two interactions, from the Fly stage -----------------
await page.keyboard.press('Escape')
await page.waitForTimeout(250)
let interactions = 0
await page.locator('[data-testid="flight-menu"]').click()
interactions += 1
await hold(page.locator('[data-testid="flight-cmd-rtl"]'), HOLD_AT_MS)
interactions += 1
const rtl = await commands()
check(
  rtl.some((c) => c.request.intent.kind === 'return_to_launch'),
  'RTL goes out from the flight stage',
)
check(interactions <= 2, `RTL costs at most two interactions (got ${interactions})`)
await page.evaluate(() => window.__mockFlightReply('return_to_launch'))
await page.keyboard.press('Escape')
await page.waitForTimeout(250)

// --- 7. the keyboard is a first-class hold path -----------------------------
const armCount = async () =>
  (await commands()).filter((c) => c.request.intent.kind === 'arm' || c.request.intent.kind === 'disarm').length
const armButton = page.locator('[data-testid="flight-cmd-arm_disarm"]')
await page.locator('[data-testid="flight-menu"]').click()
await armButton.focus()
await page.keyboard.down('Enter')
await page.waitForTimeout(HOLD_AT_MS)
await page.keyboard.up('Enter')
check((await armCount()) === 1, 'holding Enter sends arm/disarm')
await page.evaluate(() => {
  window.__mockArm(false)
  window.__mockFlightReply('disarm')
})
check(await waitUntil(async () => (await status.getAttribute('data-command')) === 'Disarm' && (await status.innerText()).includes('Accepted'), 5000),
  'Disarm acceptance keeps the requested action after telemetry becomes disarmed')
await page.screenshot({ path: join(shotsDir, 'command-disarm-accepted.png') })
await page.evaluate(() => window.__mockArm(true))
await page.keyboard.press('Escape')
await page.waitForTimeout(250)
check((await armCount()) === 1, 'a single Enter press after the hold does not send again')

// --- 8. mode switching follows the same grading -----------------------------
await page.locator('[data-testid="mode-menu"]').click()
const setModeCount = async () =>
  (await commands()).filter((c) => c.request.intent.kind === 'set_mode').length
check((await page.locator('[data-testid="confirm-bar"]').count()) === 1, 'the mode popover carries the confirm bar')

// Low risk (Hold): one click.
await page.locator('[data-testid="mode-hold"]').click()
check(await waitUntil(async () => (await setModeCount()) === 1, 5000), 'Hold switches on a single click')
const firstMode = (await commands()).find((c) => c.request.intent.kind === 'set_mode')
check(
  firstMode?.request.intent.main_mode === 4 && firstMode?.request.intent.sub_mode === 3,
  `the switch carries the PX4 mode pair (got ${firstMode?.request.intent.main_mode}/${firstMode?.request.intent.sub_mode})`,
)
await page.evaluate(() => {
  window.__mockFlightReply('set_mode')
  window.__mockMode(4, 3, 'Hold')
})
await waitUntil(async () => (await page.locator('[data-testid="mode-switching"]').count()) === 0, 8000)

// Medium risk (Return): a click is not enough, a hold is.
await page.locator('[data-testid="mode-return"]').click()
await page.waitForTimeout(300)
check((await setModeCount()) === 1, 'a single click cannot switch into Return')
await hold(page.locator('[data-testid="mode-return"]'), HOLD_AT_MS)
check(await waitUntil(async () => (await setModeCount()) === 2, 5000), 'holding Return switches the mode')
const returnMode = (await commands()).filter((c) => c.request.intent.kind === 'set_mode')[1]
check(
  returnMode?.request.intent.main_mode === 4 && returnMode?.request.intent.sub_mode === 5,
  `Return sends the RTL mode pair (got ${returnMode?.request.intent.main_mode}/${returnMode?.request.intent.sub_mode})`,
)
await page.evaluate(() => {
  window.__mockFlightReply('set_mode')
  window.__mockMode(4, 5, 'RTL')
})

// --- 9. Runtime identity scopes every request and retires stale replies -----
const beforeStale = (await commands()).length
await hold(page.locator('[data-testid="rtl-always"]'), HOLD_AT_MS)
check(
  await waitUntil(async () => (await commands()).length === beforeStale + 1, 5000),
  'a command is pending before the target replacement',
)
await page.evaluate(() => {
  window.__mockSetRuntimeTarget(2)
  window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: false })
})
check(await waitUntil(async () => (await status.count()) === 0, 5000), 'target loss clears the old command scope')
await page.evaluate(() => window.__mockFlightReply('return_to_launch'))
await page.waitForTimeout(300)
check((await status.count()) === 0, 'an old-target operation reply is ignored')

const scoped = await commands()
const requestIds = scoped.map((entry) => entry.request?.context?.request_id)
check(
  scoped.every((entry) =>
    entry.request?.context?.session?.module_id === 'flight-controls'
    && typeof entry.request.context.request_id === 'string'
    && entry.request.context.request_id.length > 0),
  'every execute carries a flight-controls session and request identity',
)
check(new Set(requestIds).size === requestIds.length, 'every execute uses a fresh request identity')
const sessionKeys = scoped.map((entry) => JSON.stringify(entry.request.context.session))
check(new Set(sessionKeys).size === 1, 'commands on one target reuse the exact Runtime session')

// --- 10. same-target old outcomes cannot settle a newer emergency -----------
await page.keyboard.press('Escape')
await page.evaluate(async () => {
  const { useCommandStore } = await import('/src/stores/command.ts')
  const { useTerrainStore } = await import('/src/stores/terrain.ts')
  const { useUiStore } = await import('/src/stores/ui.ts')
  window.__flightStore = useCommandStore
  window.__terrainStore = useTerrainStore
  window.__uiStore = useUiStore
  window.__originalTerrainSample = useTerrainStore.getState().samplePointAmsl
})
async function readyTarget(revision) {
  await page.evaluate((revision) => {
    window.__mockSetRuntimeTarget(revision)
    window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: true })
    window.__mockTelemetry(Date.now())
    window.__mockFeedStart()
  }, revision)
  await page.waitForTimeout(250)
  if (await targetConfirm.isVisible()) await targetConfirm.click()
}
await readyTarget(2)
const beforeRace = (await commands()).length
await page.evaluate(() => { void window.__flightStore.getState().sendGoto(49.2832, -123.1208) })
check(await waitUntil(async () => (await commands()).length === beforeRace + 1, 5000), 'a normal map command is pending')
await hold(page.locator('[data-testid="rtl-always"]'), HOLD_AT_MS)
check(await waitUntil(async () => (await commands()).length === beforeRace + 2, 5000), 'RTL is sent while the normal map command is pending')
await page.evaluate(() => window.__mockFlightReply('goto'))
check(await waitUntil(async () => (await status.innerText()).includes('RTL') && (await status.innerText()).includes('Sent — awaiting ack'), 5000),
  'an old same-target accepted reply cannot clear the newer RTL wait')
await page.screenshot({ path: join(shotsDir, 'command-request-isolation.png') })
await page.evaluate(() => window.__mockFlightReply('return_to_launch', 'denied'))
check(await waitUntil(async () => (await status.innerText()).includes('Denied'), 5000), 'the newer RTL retains its own denied verdict')

// --- 11. map markers belong to their individual request --------------------
const beforeHomes = (await commands()).length
await page.evaluate(() => { void window.__flightStore.getState().sendSetHome(49.2832, -123.1208) })
await waitUntil(async () => (await commands()).length === beforeHomes + 1, 5000)
await page.evaluate(() => { void window.__flightStore.getState().sendSetHome(49.2833, -123.1209) })
await waitUntil(async () => (await commands()).length === beforeHomes + 2, 5000)
await page.evaluate(() => window.__mockFlightReply('set_home'))
check(await page.evaluate(() => window.__flightStore.getState().homeMarker === null), 'an older same-target Set home reply cannot move the newer marker')
check(await page.evaluate(() => window.__flightStore.getState().pending === 'set_home'), 'an older Set home reply cannot clear the newer Set home wait')
await page.evaluate(() => window.__mockFlightReply('set_home'))
check(await waitUntil(async () => page.evaluate(() => window.__flightStore.getState().homeMarker?.[0] === 49.2833), 5000),
  'the current Set home acceptance installs only its clicked marker')
await page.screenshot({ path: join(shotsDir, 'command-home-owner.png') })

const beforeGotos = (await commands()).length
await page.evaluate(() => { void window.__flightStore.getState().sendGoto(49.2832, -123.1208) })
await waitUntil(async () => (await commands()).length === beforeGotos + 1, 5000)
await page.evaluate(() => { void window.__flightStore.getState().sendGoto(49.2833, -123.1209) })
await waitUntil(async () => (await commands()).length === beforeGotos + 2, 5000)
await page.evaluate(() => window.__mockFlightReply('goto', 'denied'))
check(await page.evaluate(() => window.__flightStore.getState().gotoTarget?.[0] === 49.2833), 'an older Go to denial cannot clear a newer Go to marker')
check(await page.evaluate(() => window.__flightStore.getState().pending === 'goto'), 'an older Go to denial cannot settle a newer Go to request')
await page.evaluate(() => window.__mockFlightReply('goto'))

// --- 12. slow terrain preparation cannot migrate to a replacement target ---
const beforeTerrain = (await commands()).length
await page.evaluate(() => {
  window.__terrainStore.setState({ samplePointAmsl: () => new Promise((resolve) => { window.__resolveTerrain = resolve }) })
  void window.__flightStore.getState().send('land')
})
check(await waitUntil(async () => page.evaluate(() => typeof window.__resolveTerrain === 'function'), 5000),
  'landing captures its Runtime context before awaiting terrain')
await page.evaluate(() => {
  window.__mockSetRuntimeTarget(3)
  window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: false })
  window.__resolveTerrain(42)
  window.__terrainStore.setState({ samplePointAmsl: window.__originalTerrainSample })
})
await page.waitForTimeout(300)
check((await commands()).length === beforeTerrain, 'obsolete terrain preparation sends zero flight commands')
check(await page.evaluate(() => window.__flightStore.getState().pending === null && window.__flightStore.getState().homeMarker === null),
  'target replacement clears old pending and home-marker authority')
await page.screenshot({ path: join(shotsDir, 'command-stale-terrain.png') })

// --- 13. a delayed old-target checklist cannot revive a refusal ------------
await readyTarget(3)
const beforeChecklist = (await commands()).length
await page.evaluate(() => {
  window.__mockArm(false)
  window.__mockDeferChecklist = true
  void window.__flightStore.getState().send('takeoff')
})
check(await waitUntil(async () => page.evaluate(() => window.__mockPendingChecklistCount() === 1), 5000), 'takeoff is waiting on its captured target checklist')
await page.evaluate(() => {
  window.__mockSetRuntimeTarget(4)
  window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: false })
  window.__mockResolveChecklist()
  window.__mockDeferChecklist = false
})
await page.waitForTimeout(300)
check((await commands()).length === beforeChecklist, 'obsolete checklist preparation sends zero takeoff commands')
check(await page.evaluate(() => window.__flightStore.getState().preflight === null), 'a delayed old-target checklist cannot reopen preflight')
await page.screenshot({ path: join(shotsDir, 'command-stale-checklist.png') })

// --- 14. native generation loss is authoritative without UI link events ----
await readyTarget(4)
const beforeHidden = (await commands()).length
await page.evaluate(() => { void window.__flightStore.getState().sendSetMode(4, 3) })
await waitUntil(async () => (await commands()).length === beforeHidden + 1, 5000)
await page.evaluate(() => {
  window.__mockFeedStop()
  window.__mockSetRuntimeTarget(5)
  window.__mockFlightReply('set_mode')
})
check(await waitUntil(async () => (await status.count()) === 0, 5000), 'a reply from a silently replaced native target cannot establish acceptance')
check(await page.evaluate(() => window.__flightStore.getState().homeMarker === null), 'native reply revalidation also retires old map authority')
await page.screenshot({ path: join(shotsDir, 'command-native-target-loss.png') })

// Discovering a later replacement while preparing a click must not replay it.
await readyTarget(5)
const beforeDiscovery = (await commands()).length
await page.evaluate(() => { void window.__flightStore.getState().send('rtl') })
await waitUntil(async () => (await commands()).length === beforeDiscovery + 1, 5000)
await page.evaluate(() => window.__mockFlightReply('return_to_launch'))
await waitUntil(async () => page.evaluate(() => window.__flightStore.getState().pending === null), 5000)
await page.evaluate(() => {
  window.__mockFeedStop()
  window.__mockSetRuntimeTarget(6)
  void window.__flightStore.getState().sendSetMode(4, 3)
})
check(await waitUntil(async () => (await status.innerText()).includes('flight target changed'), 5000), 'discovering a replaced target requires a fresh operator command')
check((await commands()).length === beforeDiscovery + 1, 'the click that discovers a replacement produces zero writes')
check((await status.getAttribute('title')).includes('issue a new command for the current target'),
  'a truncated native error retains its complete reason in the status tooltip')
for (const width of [1024, 1280, 1440, 1920]) {
  await page.setViewportSize({ width, height: 900 })
  await page.waitForTimeout(200)
  const geometry = await page.evaluate(() => {
    const ids = ['topbar-status', 'rtl-always', 'hold-always', 'project-pill']
    return ids.map((id) => {
      const rect = document.querySelector(`[data-testid="${id}"]`).getBoundingClientRect()
      return { id, left: rect.left, right: rect.right, height: rect.height, inside: rect.left >= 0 && rect.right <= innerWidth }
    })
  })
  check(geometry.every((entry) => entry.inside), `a long command error keeps status, emergency controls and app context inside ${width}px (${JSON.stringify(geometry)})`)
  await page.screenshot({ path: join(shotsDir, `command-long-error-${width}.png`) })
}
await page.setViewportSize({ width: 1440, height: 900 })
await page.screenshot({ path: join(shotsDir, 'command-replaced-target.png') })
await page.evaluate(() => { void window.__flightStore.getState().sendSetMode(4, 3) })
check(await waitUntil(async () => (await commands()).length === beforeDiscovery + 2, 5000), 'a fresh command binds the new target without replaying old intent')
await page.evaluate(() => window.__mockFlightReply('set_mode'))
check(await waitUntil(async () => (await status.innerText()).includes('Accepted'), 5000), 'the new target command accepts only its own operation reply')
check(
  await waitUntil(
    async () => page.evaluate(() => window.__flightStore.getState().pending === null),
    5000,
  ),
  'the final mode reply releases the command slot before flight-target editing',
)

const allScoped = await commands()
check(new Set(allScoped.map((entry) => entry.request.context.request_id)).size === allScoped.length,
  'all stale/racing/new-target executes retain distinct request identities')
check(allScoped.at(-1).request.context.session.target.generation === '6', 'the final fresh request names the replacement generation explicitly')

// --- 13. flight-target dialog: altitude / speed from the map menu -----------
// The map's right-click menu opens this dialog through `useUiStore`; the
// headless harness has no map, so the store is the entry point here.
const targetDialog = page.locator('[data-testid="flight-target-dialog"]')
const targetInput = page.locator('[data-testid="flight-target-input"]')
const targetSend = page.locator('[data-testid="flight-target-send"]')
const targetReason = page.locator('[data-testid="flight-target-reason"]')
const openTarget = (mode) => page.evaluate((m) => window.__uiStore.getState().openFlightTarget(m), mode)
const closeTarget = () => page.evaluate(() => window.__uiStore.getState().closeFlightTarget())

const beforeTarget = (await commands()).length
await page.evaluate(() => window.__mockArm(true))
await openTarget('speed')
await targetDialog.waitFor({ state: 'visible', timeout: 5000 })
check((await page.locator('[data-testid="flight-target-current"]').innerText()).includes('m/s'),
  'the speed dialog shows the live groundspeed')
await targetInput.fill('40')
check(await waitUntil(async () => (await targetReason.innerText()).includes('1–15'), 5000),
  'an out-of-envelope speed is refused before the wire')
check(await targetSend.isDisabled(), 'the send stays disabled outside the envelope')
await targetInput.fill('3.5')
check(
  await waitUntil(async () => !(await targetSend.isDisabled()), 5000),
  'an armed in-envelope speed enables hold-to-send',
)
await page.waitForTimeout(600)
check(await targetInput.inputValue() === '3.5',
  'live telemetry cannot overwrite an in-progress target edit')
await targetSend.click()
await page.waitForTimeout(300)
check((await commands()).length === beforeTarget, 'a single click cannot send a medium-risk flight-target command')
await hold(targetSend, HOLD_AT_MS)
check(await waitUntil(async () => (await commands()).length === beforeTarget + 1, 5000),
  'a held send issues the speed command')
const speedIntent = (await commands()).at(-1).request.intent
check(speedIntent.kind === 'change_speed' && speedIntent.speed_m_s === 3.5,
  'the speed intent carries the typed groundspeed')
check((await targetDialog.count()) === 0, 'the dialog closes once the command is sent')
await page.evaluate(() => window.__mockFlightReply('change_speed'))
check(await waitUntil(async () => (await status.innerText()).includes('Accepted'), 5000),
  'the accepted speed change settles in the status strip')

const beforeAltitude = (await commands()).length
await openTarget('altitude')
await targetDialog.waitFor({ state: 'visible', timeout: 5000 })
check((await page.locator('label[for="flight-target-value"]').innerText()).includes('AMSL'),
  'without DEM data the altitude input explicitly uses AMSL')
check(await targetInput.inputValue() === '48.5',
  'the AMSL fallback starts at the FC-reported absolute altitude')
check((await page.locator('[data-testid="flight-target-current"]').innerText()) === '48.5 m',
  'the current fallback readout shows reported AMSL, not HOME-relative altitude')
await targetInput.fill('1250')

// A DEM arriving during an edit must not change the units of that edit.
await page.evaluate(() => {
  window.__terrainStore.setState({
    tile: {
      path: '/mock/dem', folder: '/mock/dem', version: 'mock', has_dsm: false,
      bounds: { north: 50, south: 49, east: -123, west: -124 },
      source: 'hrdem', resolution_m: 1, acquisition: null, vertical_datum: 'amsl_egm96',
    },
    loading: false,
    vehicleGroundAmslM: 8.5,
    samplePointAmsl: async () => 8.5,
    sampleDem: async (points) => points.map(() => 8.5),
  })
})
await page.waitForTimeout(600)
check(await targetInput.inputValue() === '1250'
  && (await page.locator('label[for="flight-target-value"]').innerText()).includes('AMSL'),
  'a later DEM load and live telemetry preserve the edited AMSL value/reference')
check(!(await targetSend.isDisabled()), 'AMSL values above 120 m are not capped by the AGL envelope')
await targetDialog.screenshot({ path: join(flightShotsDir, 'flight-target-altitude-amsl.png') })
await hold(targetSend, HOLD_AT_MS)
check(await waitUntil(async () => (await commands()).length === beforeAltitude + 1, 5000),
  'a held send issues the fallback altitude command')
const amslIntent = (await commands()).at(-1).request.intent
check(amslIntent.kind === 'change_altitude' && amslIntent.target.kind === 'amsl'
  && amslIntent.target.altitude.datum === 'AMSL_EGM96' && amslIntent.target.altitude.meters === 1250,
  'the fallback intent carries a datum-tagged absolute target without a ground offset')
await page.evaluate(() => window.__mockFlightReply('change_altitude'))
check(await waitUntil(async () => (await status.innerText()).includes('Accepted'), 5000),
  'the accepted fallback change settles in the status strip')

const beforeAgl = (await commands()).length
await openTarget('altitude')
await targetDialog.waitFor({ state: 'visible', timeout: 5000 })
check(await waitUntil(async () => (await page.locator('label[for="flight-target-value"]').innerText()).includes('AGL'), 5000),
  'a ready DEM sample selects AGL on the next altitude edit')
await targetInput.fill('1250')
check(await targetSend.isDisabled(), 'the 1–120 m envelope still applies to an AGL target')
await targetInput.fill('30')
check(
  await waitUntil(async () => !(await targetSend.isDisabled()), 5000),
  'an armed in-envelope AGL target enables hold-to-send',
)
await targetDialog.screenshot({ path: join(flightShotsDir, 'flight-target-altitude-agl.png') })
await hold(targetSend, HOLD_AT_MS)
check(await waitUntil(async () => (await commands()).length === beforeAgl + 1, 5000),
  'a held send issues the altitude command')
const altitudeIntent = (await commands()).at(-1).request.intent
check(altitudeIntent.kind === 'change_altitude' && altitudeIntent.target.kind === 'agl' && altitudeIntent.target.agl_m === 30,
  'the altitude intent carries the typed AGL target')
await page.evaluate(() => window.__mockFlightReply('change_altitude'))
check(await waitUntil(async () => (await status.innerText()).includes('Accepted'), 5000),
  'the accepted altitude change settles in the status strip')

// Loading, uncovered/nodata, and mesh-only ground are all AMSL fallbacks.
for (const scenario of ['loading', 'nodata', 'mesh-only']) {
  await page.evaluate((scenario) => {
    window.__terrainStore.setState({
      loading: scenario === 'loading',
      ...(scenario === 'mesh-only' ? { tile: null } : {}),
      sampleDem: async (points) => points.map(() => null),
    })
  }, scenario)
  await openTarget('altitude')
  await targetDialog.waitFor({ state: 'visible', timeout: 5000 })
  check(await waitUntil(async () => !(await targetInput.isDisabled()), 5000), `${scenario}: DEM resolution finishes`)
  check((await page.locator('label[for="flight-target-value"]').innerText()).includes('AMSL'),
    `${scenario}: stale mesh/AGL data cannot stand in for a ready DTM`)
  await closeTarget()
}

// Unarmed: the dialog names the gate and refuses before any write.
const beforeUnarmed = (await commands()).length
await page.evaluate(() => window.__mockArm(false))
await openTarget('altitude')
await targetDialog.waitFor({ state: 'visible', timeout: 5000 })
check((await targetReason.innerText()).includes('Arm'), 'the altitude dialog names the armed gate')
check(await targetSend.isDisabled(), 'an unarmed altitude change cannot be sent')
await targetDialog.screenshot({ path: join(flightShotsDir, 'flight-target-altitude-unarmed.png') })
await targetSend.click({ force: true })
await page.waitForTimeout(300)
check((await commands()).length === beforeUnarmed, 'the refused altitude change writes nothing')
await page.evaluate(() => window.__mockArm(true))
await closeTarget()

const unhandled = await page.evaluate(() => window.__mockUnhandled)
if (unhandled.length > 0) {
  console.log(`  unmocked commands (ignored): ${[...new Set(unhandled)].join(', ')}`)
}

await browser.close()

if (pageErrors.length > 0) {
  console.error('page errors:', pageErrors.slice(0, 8))
  process.exit(2)
}
if (failed > 0) {
  console.error(`check:command-ux FAILED — ${failed} assertion(s)`)
  process.exit(1)
}
console.log('check:command-ux OK')
