// End-to-end check for the U5 preflight checklist surface.
//   node check-preflight.mjs <viteUrl> <shotsDir>
//
// The fake shell intercepts `preflight_checklist`, the Runtime client API and
// `flight_command_execute`:
//   - the checklist is `window.__mockChecklist` (a blocked one by default)
//   - an ungated arm rejects with the structured `PreflightRefusal` the real
//     backend returns when the gate blocks; a forced arm (force payload)
//     resolves and records the request
//
// Checks:
//   1. the FlightMenu Preflight entry opens the full checklist (groups + badge)
//   2. a blocked arm auto-opens the panel with the reason + force section
//   3. the wrong keyword cannot confirm; `FORCE ARM` enables it
//   4. the forced arm sends exactly one typed Arm intent with the force
//      payload, then the panel closes
//   5. a warn-only checklist requires per-item confirmation before arming
import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node check-preflight.mjs <viteUrl> <shotsDir>')
  process.exit(1)
}

const HOLD_MS = 1000
const HOLD_AT_MS = 1600

let failed = 0
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`)
  if (!ok) failed += 1
}

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
  const pack = (main, sub) => ((main & 0xff) << 16) | ((sub & 0xff) << 24)
  let armed = false
  let feedTimer = undefined

  const snapshot = (atMs) => ({
    last_heartbeat_at_ms: atMs % 100000,
    heartbeat: {
      system_id: 1,
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
      custom_mode: pack(4, 3),
      mode_label: 'Hold',
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
      extended_sys_state_at_ms: atMs,
      home_at_ms: atMs,
      estimator_at_ms: null,
      vibration_at_ms: null,
      rc_channels_at_ms: null,
      radio_at_ms: null,
    },
    extended_sys_state: { landed_state: 'in_air', vtol_state: 'undefined' },
    home: { latitude_deg: 49.2831, longitude_deg: -123.1207, altitude: { datum: 'AMSL_EGM96', meters: 8.5 } },
    estimator: null,
    vibration: null,
    rc_channels: null,
    radio: null,
  })

  const item = (id, group, status, detail, fixHint = null) => ({
    id,
    group,
    status,
    detail,
    fix_hint: fixHint,
  })

  // A blocked checklist: the vehicle reports in_air, so arming is refused.
  const blocked = {
    items: [
      item('link_fresh', 'link', 'pass', 'Flight controller is reporting'),
      item('gps_fix', 'navigation', 'pass', '18 satellites, 3D fix'),
      item('rtk', 'navigation', 'pass', 'RTK not required'),
      item('home_position', 'navigation', 'pass', 'Home position set'),
      item('battery', 'power', 'pass', 'Battery 76%'),
      item('sensor_health', 'system', 'pass', 'All enabled sensors report healthy'),
      item('landed_on_ground', 'system', 'fail', 'Vehicle is airborne', 'Land and disarm first'),
      item('mission_ready', 'mission', 'pass', 'Mission uploaded and verified'),
      item('flight_mode_ready', 'system', 'pass', 'Flight controller ready (Hold)'),
      item('motor_test_idle', 'system', 'pass', 'Motor test not running'),
      item('dem_coverage', 'terrain', 'fail', 'The DEM does not cover the whole route', 'Prepare the survey area'),
      item('drape_thresholds', 'terrain', 'fail', 'No DSM clearance entered', 'Enter the minimum DSM clearance'),
      item('drape_infeasible', 'terrain', 'fail', '2 out-of-band span(s)', 'Review the spans'),
    ],
  }
  const warnOnly = {
    items: blocked.items.map((i) => {
      if (i.id === 'gps_fix') return { ...i, status: 'warn', detail: '2D fix only', fix_hint: 'Wait for a 3D fix' }
      if (i.id === 'landed_on_ground') return { ...i, status: 'pass', detail: 'On ground' }
      if (i.group === 'terrain') return { ...i, status: 'pass', detail: i.detail.replace('does not cover', 'covers') }
      return i
    }),
  }

  window.__mockCommands = []
  window.__mockUnhandled = []
  const target = {
    vehicle_id: 'preflight-vehicle',
    session_id: 'preflight-target-session',
    link_id: 'preflight-link',
    generation: '1',
    mavlink_target: { system_id: 1, component_id: 1 },
  }
  const runtimeSession = {
    owner: { client_id: 'preflight-client', instance_id: 'preflight-flight-controls' },
    target,
    module_id: 'flight-controls',
  }
  let operation = 0
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b)
  const flightReply = (request, result = 'accepted') => ({
    operation: {
      id: `preflight-operation-${++operation}`,
      owner: structuredClone(request.context.session.owner),
      target: structuredClone(request.context.session.target),
    },
    command: request.intent.kind,
    steps: [{ step: request.intent.kind, result }],
  })
  window.__mockChecklist = 'blocked' // 'blocked' | 'warn' | 'clean'
  window.__mockArmOutcome = 'accepted' // 'accepted' | 'denied' | 'uncertain'
  window.__mockFeedStart = (intervalMs = 250) => {
    clearInterval(feedTimer)
    window.__mockTelemetry(Date.now())
    feedTimer = setInterval(() => window.__mockTelemetry(Date.now()), intervalMs)
  }
  window.__mockFeedStop = () => clearInterval(feedTimer)
  window.__mockTelemetry = (atMs) => deliver('telemetry', snapshot(atMs))
  window.__mockLink = (status) => deliver('link', status)
  window.__mockSetChecklist = (mode) => {
    window.__mockChecklist = mode
    window.__mockTelemetry(Date.now())
  }
  window.__mockArm = (next) => {
    armed = next
    window.__mockTelemetry(Date.now())
  }
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
      if (cmd === 'flight_mode_table') return []
      if (cmd === 'runtime_targets') {
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
      if (cmd === 'runtime_client_attach') return structuredClone(runtimeSession)
      if (cmd === 'runtime_client_validate') {
        if (!same(args.session, runtimeSession)) throw { code: 'not_owner' }
        return null
      }
      if (cmd === 'runtime_client_detach') return null
      if (cmd === 'preflight_checklist') {
        window.__mockCommands.push({ cmd, ...(args ?? {}) })
        return window.__mockChecklist === 'warn' ? warnOnly : window.__mockChecklist === 'clean' ? { items: [] } : blocked
      }
      if (cmd === 'flight_command_execute') {
        const entry = { cmd, ...(args ?? {}) }
        window.__mockCommands.push(entry)
        const intent = args.request.intent
        if (intent.kind === 'arm' && !intent.force && window.__mockChecklist === 'blocked') {
          throw {
            code: 'preflight',
            refusal: {
              kind: 'blocked',
              items: blocked.items.filter((i) => i.status === 'fail' || i.status === 'unknown'),
              message: 'preflight blocks arming',
            },
          }
        }
        if (intent.kind === 'arm' && window.__mockArmOutcome === 'denied') {
          return flightReply(args.request, 'denied')
        }
        if (intent.kind === 'arm' && window.__mockArmOutcome === 'uncertain') {
          throw {
            code: 'command',
            error: {
              code: 'reconciliation_required',
              operation: flightReply(args.request).operation,
              reason: 'Local retries retired; FC arm outcome is unconfirmed',
            },
          }
        }
        if (intent.kind === 'arm') {
          armed = true // a successful arm (normal, acked-warn or forced) arms the vehicle
          window.__mockTelemetry(Date.now())
        }
        if (intent.kind === 'disarm') {
          armed = false
          window.__mockTelemetry(Date.now())
        }
        return flightReply(args.request)
      }
      if (cmd === 'actuator_stop') {
        window.__mockCommands.push({ cmd })
        return null
      }
      window.__mockCommands.push({ cmd, ...(args ?? {}) })
      window.__mockUnhandled.push(cmd)
      return null
    },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
})

await page.goto(`${url}` + (url.includes('?') ? '&' : '?') + 'nomap', { waitUntil: 'networkidle', timeout: 60000 })
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
await waitUntil(async () => (await page.locator('[data-testid="flight-menu"]').count()) === 1, 20000)

const armCalls = () =>
  page.evaluate(() => window.__mockCommands.filter((c) =>
    c.cmd === 'flight_command_execute' && c.request.intent.kind === 'arm'))
const dialog = page.locator('[data-testid="preflight-dialog"]')

// --- 1. the FlightMenu Preflight entry opens the full checklist ------------
await page.locator('[data-testid="flight-menu"]').click()
await page.locator('[data-testid="flight-cmd-preflight"]').click()
check(await waitUntil(async () => (await dialog.count()) === 1, 5000), 'the Preflight entry opens the checklist panel')
check(
  await waitUntil(async () => (await page.locator('[data-testid="preflight-ready"]').count()) === 1, 5000),
  'the readiness badge renders',
)
const readyText = await page.locator('[data-testid="preflight-ready"]').innerText()
check(/9\/13/.test(readyText), `the badge counts the passing items (got "${readyText}")`)
check(
  (await page.locator('[data-testid="preflight-dialog"]').getByText('Land and disarm first').count()) === 1,
  'the failed item carries its fix hint',
)
await page.screenshot({ path: join(shotsDir, 'preflight-panel.png') })
await page.locator('[data-testid="preflight-cancel"]').click()

// --- 2. a blocked arm auto-opens the panel with the force section -----------
await page.locator('[data-testid="flight-menu"]').click()
await page.locator('[data-testid="flight-cmd-arm_disarm"]').hover()
await page.mouse.down()
await page.waitForTimeout(HOLD_AT_MS)
await page.mouse.up()
check(
  await waitUntil(async () => (await dialog.count()) === 1, 5000),
  'a blocked arm auto-opens the preflight panel',
)
check(
  (await dialog.innerText()).includes('preflight blocks arming'),
  'the refusal reason is shown',
)
check((await page.locator('[data-testid="preflight-keyword"]').count()) === 1, 'the force-arm keyword entry appears')
const confirm = page.locator('[data-testid="preflight-confirm"]')
check((await confirm.isDisabled()) === true, 'the force confirm is disabled before the keyword matches')

// --- 3. wrong keyword stays disabled; FORCE ARM enables it -------------------
await page.locator('[data-testid="preflight-keyword"]').fill('FORCE')
check((await confirm.isDisabled()) === true, 'a partial keyword cannot confirm')
await page.locator('[data-testid="preflight-keyword"]').fill('FORCE ARM')
check(await waitUntil(async () => !(await confirm.isDisabled()), 3000), 'the verbatim keyword enables the force confirm')
await page.screenshot({ path: join(shotsDir, 'preflight-force-ready.png') })

// --- 4. the forced arm sends exactly one command with the force payload -----
await page.evaluate(() => { window.__mockArmOutcome = 'denied' })
const beforeDenied = (await armCalls()).length
await confirm.click()
check(await waitUntil(async () => (await armCalls()).length === beforeDenied + 1, 5000),
  'one confirmation sends exactly one forced arm attempt')
check(await waitUntil(async () => (await dialog.innerText()).includes('denied'), 5000),
  'a denied forced arm keeps the panel and its verdict visible')
check(await confirm.isEnabled(), 'a denied arm restores confirmation instead of remaining busy')
await confirm.scrollIntoViewIfNeeded()
await page.screenshot({ path: join(shotsDir, 'preflight-force-denied.png') })
await page.evaluate(() => { window.__mockArmOutcome = 'uncertain' })
await confirm.click()
check(await waitUntil(async () => (await dialog.innerText()).includes('FC arm outcome is unconfirmed'), 5000),
  'an uncertain forced arm keeps the reconciliation reason visible')
check(await confirm.isEnabled(), 'an uncertain arm restores confirmation')
await confirm.scrollIntoViewIfNeeded()
await page.screenshot({ path: join(shotsDir, 'preflight-force-uncertain.png') })
await page.evaluate(() => { window.__mockArmOutcome = 'accepted' })
const beforeAccepted = (await armCalls()).length
await confirm.click()
check(
  await waitUntil(async () => (await armCalls()).some((c) => c.request.intent.force?.reason === 'safety_override'), 5000),
  'the forced arm carries the chosen reason',
)
const sent = await armCalls()
const forced = sent.filter((c) => c.request.intent.force)
check(sent.length === beforeAccepted + 1, 'successful confirmation sends exactly one additional forced arm')
check(forced.length === 3, `denied, uncertain and accepted attempts have separate requests (got ${forced.length})`)
check(
  forced[0].request.intent.force?.note === null || typeof forced[0].request.intent.force?.note === 'string',
  'the note is optional',
)
check(await waitUntil(async () => (await dialog.count()) === 0, 5000), 'the panel closes after a successful forced arm')
check(await page.locator('[data-testid="command-status"]').getAttribute('data-command') === 'Arm',
  'Arm acceptance keeps the requested action after telemetry becomes armed')
await page.screenshot({ path: join(shotsDir, 'preflight-force-accepted.png') })

// --- 4b. the armed vehicle may Take off past the gate -----------------------
// The forced arm armed the vehicle; Takeoff must not re-block with the same
// checklist (the arm decision stands, PX4 takeoff handling applies).
await page.evaluate(() => window.__mockSetChecklist('blocked'))
await page.waitForTimeout(300)
await page.locator('[data-testid="flight-menu"]').click()
const tk = page.locator('[data-testid="flight-cmd-takeoff"]')
await tk.hover()
await page.mouse.down()
await page.waitForTimeout(HOLD_AT_MS)
await page.mouse.up()
const takeoffState = await page.evaluate(() => ({
  dialog: document.querySelectorAll('[data-testid="preflight-dialog"]').length,
  cmds: window.__mockCommands.slice(-4).map((c) => ({ cmd: c.cmd, kind: c.request?.intent?.kind })),
  armed: window.__mockCommands.some((c) => c.cmd === 'flight_command_execute' && c.request.intent.kind === 'arm'),
}))
console.log('takeoff state:', JSON.stringify(takeoffState))
check(
  await waitUntil(
    async () =>
      (await page.evaluate(() =>
        window.__mockCommands.some((c) => c.cmd === 'flight_command_execute' && c.request.intent.kind === 'takeoff'),
      )) === true,
    5000,
  ),
  'takeoff proceeds after the forced arm (gate does not re-block)',
)
const takeoffRequest = await page.evaluate(() =>
  window.__mockCommands.find((c) => c.cmd === 'flight_command_execute' && c.request.intent.kind === 'takeoff'))
check(takeoffRequest?.request.intent.plan_state != null, 'takeoff carries the current plan state')
check(
  Array.isArray(takeoffRequest?.request.intent.acknowledged_warnings)
    && takeoffRequest.request.intent.acknowledged_warnings.length === 0,
  'the current takeoff menu carries an empty warning acknowledgement list',
)

// --- 5. a warn-only checklist needs per-item confirmation -------------------
await page.keyboard.press('Escape')
await page.evaluate(() => window.__mockSetChecklist('warn'))
await page.evaluate(() => window.__mockArm(false))
await page.locator('[data-testid="flight-menu"]').click()
await page.locator('[data-testid="flight-cmd-preflight"]').click()
check(await waitUntil(async () => (await dialog.count()) === 1, 5000), 'the panel reopens for the warn checklist')
const warnBoxes = page.locator('[data-testid="preflight-dialog"] input[type="checkbox"]')
check(
  await waitUntil(async () => (await warnBoxes.count()) === 1, 5000),
  'the warn items are listed once the checklist loads',
)
check(
  (await page.locator('[data-testid="preflight-dialog"]').getByText('Confirm each warning').count()) === 1,
  'the per-item confirmation hint appears',
)
const ackArm = page.locator('[data-testid="preflight-confirm"]')
check((await ackArm.count()) === 0 || (await ackArm.isDisabled()) === true, 'arming stays disabled until every warning is confirmed')
await warnBoxes.first().check()
check(
  await waitUntil(async () => (await ackArm.isDisabled()) === false, 3000),
  'confirming every warning enables the arm',
)
const beforeWarn = (await armCalls()).length
await ackArm.click()
check(
  await waitUntil(async () => (await armCalls()).length === beforeWarn + 1, 5000),
  'the confirmed warn arm is sent (no force payload)',
)
const warnedArm = (await armCalls()).at(-1)
check(
  warnedArm.request.intent.force === null
    && warnedArm.request.intent.acknowledged_warnings.includes('gps_fix')
    && warnedArm.request.intent.plan_state != null,
  'Arm carries force, plan state and acknowledged warnings in one typed intent',
)
check(await waitUntil(async () => (await dialog.count()) === 0, 5000), 'the accepted warning arm closes only its own panel')
check(await page.locator('[data-testid="command-status"]').getAttribute('data-command') === 'Arm',
  'warning-arm acceptance also retains its exact requested action')
await page.screenshot({ path: join(shotsDir, 'preflight-warnings-accepted.png') })

// --- 6. U5 repair entries + the out-of-band acceptance ---------------------
// A verified plan + a solver preview with infeasible spans: the acceptance
// must flow into the next checklist evaluation as `infeasible_confirmed`.
await page.evaluate(() => {
  window.__mockSetChecklist('blocked')
  window.__mgMission.setState({
    altitudeMode: 'terrain',
    waypoints: [{
      position: { latitude_deg: 48.5, longitude_deg: -123.4 },
      altitude: { datum: 'AMSL_EGM96', meters: 100 },
      command: 16, params: [0, 0, 0, 0], autocontinue: true, locked: false,
    }],
    fcMatches: true,
    dirty: false,
    syncState: 'idle',
    drapePreview: {
      result: {
        spec_hash: 'a'.repeat(64),
        infeasible: [
          { line_id: 'S1', s_start: 10, s_end: 30, reason: 'climb_rate', max_deficit_m: 4.2 },
          { line_id: 'S1', s_start: 90, s_end: 110, reason: 'clearance', max_deficit_m: 1.5 },
        ],
        lines: [], compiled: [], crossings: [],
        stats: { agl_mean_m: 40, agl_std_m: 0.5, agl_min_m: 39, agl_max_m: 41, min_clearance_m: 5.2, max_grade: 0.6, max_climb_needed_mps: 1.2, compile_deviation_max_m: 0.4 },
        waypoints: [],
        dem_ref: { item_id: null, project_name: null, acquisition: null, hash: 'x', datum: 'OrthometricCgvd2013' },
        version: 1,
      },
      waypoints: [], landing_amsl_m: [],
      has_dtm: true, acquisition: null, resolution_m: 1, covers_route: true, dem_age_ok: true,
    },
  })
})
await page.locator('[data-testid="flight-menu"]').click()
await page.locator('[data-testid="flight-cmd-preflight"]').click()
check(await waitUntil(async () => (await dialog.count()) === 1, 5000), 'the panel re-opens for the repair-entry checks')
const fixDem = await page.locator('[data-testid="preflight-fix-dem_coverage"]').count()
const fixThresh = await page.locator('[data-testid="preflight-fix-drape_thresholds"]').count()
const terrainGroup = await page.locator('[data-testid="preflight-dialog"]').getByText('Terrain').count()
check(fixDem === 1 && fixThresh === 1, 'the terrain Fails carry repair entries')
check(terrainGroup >= 1, 'the terrain group renders')
const spansShown = await page.locator('[data-testid="preflight-dialog"]').getByText(/s 10–30 m/).count()
check(spansShown >= 1, 'the out-of-band spans are listed for review')
await page.locator('[data-testid="preflight-accept-infeasible"]').click()
await waitUntil(async () => {
  const calls = await page.evaluate(() =>
    window.__mockCommands.filter((c) => c.cmd === 'preflight_checklist'),
  )
  const last = calls[calls.length - 1]
  return last?.planState?.drape?.infeasible_confirmed === true
}, 5000)
check(true, 'the acceptance flows into the next evaluation as infeasible_confirmed')
check(
  (await page.locator('[data-testid="preflight-accept-infeasible"]').getAttribute('disabled')) !== null,
  'the accept entry is spent for this solve',
)
await page.locator('[data-testid="preflight-cancel"]').click()
// A navigation entry closes the panel and opens the drape parameters.
await page.locator('[data-testid="flight-menu"]').click()
await page.locator('[data-testid="flight-cmd-preflight"]').click()
await waitUntil(async () => (await dialog.count()) === 1, 5000)
await page.locator('[data-testid="preflight-fix-drape_thresholds"]').click()
check(await waitUntil(async () => (await dialog.count()) === 0, 5000), 'the drape-parameter entry closes the panel')
check(
  await waitUntil(
    async () =>
      (await page.evaluate(() => {
        const card = document.querySelector('[data-plan-field="drape-params"]')
        return !!card && card.getBoundingClientRect().height > 0
      })),
    5000,
  ),
  'the drape parameters are revealed on the Pattern step',
)
await page.screenshot({ path: join(shotsDir, 'preflight-u5-repairs.png') })

const scoped = await page.evaluate(() =>
  window.__mockCommands.filter((c) => c.cmd === 'flight_command_execute'))
const requestIds = scoped.map((entry) => entry.request?.context?.request_id)
check(
  scoped.every((entry) =>
    entry.request?.context?.session?.module_id === 'flight-controls'
    && typeof entry.request.context.request_id === 'string'
    && entry.request.context.request_id.length > 0),
  'every preflight execute carries its exact session and request identity',
)
check(new Set(requestIds).size === requestIds.length, 'preflight executes use fresh request identities')

await browser.close()
if (pageErrors.length > 0) {
  console.error(`  page errors: ${pageErrors.join(' | ')}`)
  process.exit(2)
}
if (failed > 0) {
  console.log(`${failed} FAILED`)
  process.exit(1)
}
console.log('check-preflight OK')
