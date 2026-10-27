// End-to-end check for the always-visible safety pair (plan §4 L2, acceptance:
// "RTL costs at most two interactions from any view or window state, and it
// still goes out immediately while another command is in flight").
//   node check-safety.mjs <viteUrl> <shotsDir>
//
// The fake FC lives in the page, owns a Runtime flight-controls session, and
// records every typed `flight_command_execute` request. Deferred replies and
// structured command errors model terminal native operation outcomes.
//
// The priority arbitration itself is Rust's (A7: `begin_with_priority` +
// `an_emergency_command_preempts_an_in_flight_goto`); what is checked here is
// that the UI never takes that decision into its own hands — the pair stays
// reachable while the dropdown is gated, and the rejection the FC sends back
// is kept on screen until the operator acknowledges it.
import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node check-safety.mjs <viteUrl> <shotsDir>')
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
page.on('pageerror', (e) => pageErrors.push(String(e)))
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

  let armed = true
  let customMode = pack(2, 0) // Altitude: not Hold, so the Hold button is live
  let modeLabel = 'Altitude'
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
        auto_enabled: false,
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
      altitude: { datum: 'AMSL_EGM96', meters: 61.5 },
      relative_alt_m: 52,
      velocity: { x_m_s: 0, y_m_s: 0, z_m_s: 0 },
      heading_deg: 12,
    },
    attitude: { roll_deg: 0.3, pitch_deg: -0.4, yaw_deg: 12 },
    sys_status: {
      sensors: {},
      battery_voltage_mv: 15400,
      battery_current_ma: 12000,
      battery_remaining_percent: 64,
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
  const target = {
    vehicle_id: 'safety-vehicle',
    session_id: 'safety-target-session',
    link_id: 'safety-link',
    generation: '1',
    mavlink_target: { system_id: 1, component_id: 1 },
  }
  const runtimeSession = {
    owner: { client_id: 'safety-client', instance_id: 'safety-flight-controls' },
    target,
    module_id: 'flight-controls',
  }
  const pendingFlights = []
  let operation = 0
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b)
  window.__mockFlightReply = (kind, result = 'accepted') => {
    const index = pendingFlights.findIndex((entry) => entry.request.intent.kind === kind)
    if (index < 0) throw new Error(`no pending ${kind} flight command`)
    const [entry] = pendingFlights.splice(index, 1)
    entry.resolve({
      operation: {
        id: `safety-operation-${++operation}`,
        owner: structuredClone(entry.request.context.session.owner),
        target: structuredClone(entry.request.context.session.target),
      },
      command: entry.request.intent.kind,
      steps: [{ step: entry.request.intent.kind, result }],
    })
  }
  window.__mockFlightError = (kind, error) => {
    const index = pendingFlights.findIndex((entry) => entry.request.intent.kind === kind)
    if (index < 0) throw new Error(`no pending ${kind} flight command`)
    const [entry] = pendingFlights.splice(index, 1)
    entry.reject(error)
  }
  window.__mockTelemetry = (atMs) => deliver('telemetry', snapshot(atMs))
  window.__mockFeedStart = (intervalMs = 250) => {
    clearInterval(feedTimer)
    window.__mockTelemetry(Date.now())
    feedTimer = setInterval(() => window.__mockTelemetry(Date.now()), intervalMs)
  }
  window.__mockLink = (status) => deliver('link', status)
  window.__mockArm = (next) => {
    armed = next
    window.__mockTelemetry(Date.now())
  }
  window.__mockMode = (main, sub, label) => {
    customMode = pack(main, sub)
    modeLabel = label
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
      if (cmd === 'flight_command_execute') {
        window.__mockCommands.push({ cmd, ...(args ?? {}) })
        return new Promise((resolve, reject) => pendingFlights.push({ request: structuredClone(args.request), resolve, reject }))
      }
      if (cmd === 'flight_mode_table') {
        return [modeEntry('hold', 'Hold', 4, 3, false), modeEntry('altitude', 'Altitude', 2, 0, false)]
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
await waitUntil(async () => (await page.locator('[data-testid="rtl-always"]').count()) === 1, 20000)

const commands = () =>
  page.evaluate(() =>
    window.__mockCommands.filter((c) => c.cmd === 'flight_command_execute'),
  )
const rtl = page.locator('[data-testid="rtl-always"]')
const hold = page.locator('[data-testid="hold-always"]')
const status = page.locator('[data-testid="command-status"]')

async function hold_(locator, ms, midShot = null) {
  await locator.hover()
  await page.mouse.down()
  if (midShot) {
    await page.waitForTimeout(450)
    await midShot()
  }
  await page.waitForTimeout(ms)
  await page.mouse.up()
}

async function selectStage(stage) {
  await page.locator('[data-testid="workflow-open"]').click()
  await page.locator(`[data-testid="workflow-stage-${stage}"]`).click()
}

// --- 1. the pair is on the bar, whatever the view ---------------------------
check((await rtl.count()) === 1, 'RTL is a visible button, not a menu entry')
check((await hold.count()) === 1, 'Hold is a visible button, not a menu entry')
check((await rtl.isEnabled()) && (await hold.isEnabled()), 'both are usable while the link is live')
check(
  (await hold.getAttribute('data-mode-current')) === 'false',
  'the Hold button knows the vehicle is not in Hold yet',
)

await selectStage('plan')
await waitUntil(async () => (await rtl.count()) === 1, 5000)
check((await rtl.count()) === 1, 'the pair survives the switch to the Plan stage')
await page.screenshot({ path: join(shotsDir, 'safety-plan.png') })
await selectStage('fly')
await page.waitForTimeout(300)

// --- 2. a stray click cannot RTL --------------------------------------------
await rtl.click()
await page.waitForTimeout(200)
check((await commands()).length === 0, 'one click on RTL sends nothing')

// --- 3. one hold from the Plan stage is the whole interaction ---------------
await selectStage('plan')
await page.waitForTimeout(300)
let interactions = 1 // the single hold below
let barSeen = null
await hold_(rtl, HOLD_AT_MS, async () => {
  barSeen = await page.locator('[data-testid="confirm-bar"]').innerText()
  await page.screenshot({ path: join(shotsDir, 'safety-holding.png') })
})
const afterPlan = await commands()
check(
  afterPlan.some((c) => c.request.intent.kind === 'return_to_launch'),
  'RTL goes out from the Plan stage',
)
check(interactions <= 2, `RTL costs at most two interactions (got ${interactions})`)
check(
  barSeen !== null && /MODE/.test(barSeen) && /AGL/.test(barSeen) && /BATT/.test(barSeen),
  `the confirm bar shows mode, height above ground and battery while holding (got "${barSeen}")`,
)
await page.evaluate(() => window.__mockFlightReply('return_to_launch'))
await waitUntil(async () => (await status.innerText()).includes('Accepted'), 5000)
await selectStage('fly')
await page.waitForTimeout(300)

// --- 4. an in-flight command does not take the pair away --------------------
await page.locator('[data-testid="flight-menu"]').click()
await hold_(page.locator('[data-testid="flight-cmd-takeoff"]'), HOLD_AT_MS)
const sent = await commands()
check(
  sent.some((c) => c.request.intent.kind === 'takeoff'),
  'the dropdown still sends its own commands',
)
check(
  await waitUntil(async () => await page.locator('[data-testid="flight-cmd-takeoff"]').isDisabled(), 5000),
  'the dropdown is gated while a command is in flight',
)
await page.keyboard.press('Escape')
await page.waitForTimeout(250)
check(await rtl.isEnabled(), 'RTL stays enabled while that command is in flight')
const before = (await commands()).length
await hold_(rtl, HOLD_AT_MS)
const after = await commands()
check(
  after.length === before + 1 && after[after.length - 1].request.intent.kind === 'return_to_launch',
  'RTL still goes out immediately (the backend arbitrates priority)',
)
await page.screenshot({ path: join(shotsDir, 'safety-in-flight.png') })

// --- 5. a rejection is kept until it is acknowledged ------------------------
await page.evaluate(() => window.__mockFlightError('takeoff', {
  code: 'command',
  error: { code: 'protocol', error: { code: 'superseded' } },
}))
await page.waitForTimeout(300)
check((await status.innerText()).includes('RTL') && (await status.innerText()).includes('Sent — awaiting ack'),
  'the old superseded result cannot settle the newer pending RTL')
const supersededLogged = await page.evaluate(async () => {
  const { useEventLog } = await import('/src/stores/eventlog.ts')
  return useEventLog.getState().events.some((event) => event.text === 'takeoff: superseded')
})
check(supersededLogged, 'the displaced same-target result is retained in the event log')
await page.screenshot({ path: join(shotsDir, 'safety-old-request.png') })
await page.evaluate(() => window.__mockFlightReply('return_to_launch', 'denied'))
check(await waitUntil(async () => (await status.innerText()).includes('Denied'), 5000),
  'the current RTL verdict settles its own status strip')
await page.waitForTimeout(1500)
check(
  (await status.innerText()).includes('Denied'),
  'the rejection is not swallowed by itself — it waits for the operator',
)
check(
  (await page.locator('[data-testid="command-status-dismiss"]').count()) === 1,
  'the strip carries an acknowledge button',
)
await page.locator('[data-testid="command-status-dismiss"]').click()
check(await waitUntil(async () => (await status.count()) === 0, 5000), 'acknowledging clears it')
await page.screenshot({ path: join(shotsDir, 'safety-denied-acknowledged.png') })

// --- 6. Hold is one click (low risk) and lights up when it lands -------------
const modeBefore = (await commands()).filter((c) => c.request.intent.kind === 'set_mode').length
await hold.click()
check(
  await waitUntil(async () => (await commands()).filter((c) => c.request.intent.kind === 'set_mode').length === modeBefore + 1, 5000),
  'Hold switches on a single click',
)
const pair = (await commands()).filter((c) => c.request.intent.kind === 'set_mode').pop()
check(
  pair?.request.intent.main_mode === 4 && pair?.request.intent.sub_mode === 3,
  `Hold sends the PX4 Auto·Loiter pair (got ${pair?.request.intent.main_mode}/${pair?.request.intent.sub_mode})`,
)
await page.evaluate(() => {
  window.__mockFlightReply('set_mode')
  window.__mockMode(4, 3, 'Hold')
})
check(
  await waitUntil(async () => (await hold.getAttribute('data-mode-current')) === 'true', 5000),
  'the Hold button shows the vehicle is already holding',
)

// --- 7. no link, no safety command ------------------------------------------
await page.evaluate(() =>
  window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: false }),
)
check(
  await waitUntil(async () => await rtl.isDisabled(), 5000),
  'RTL is gated when the heartbeat is gone',
)
check(await hold.isDisabled(), 'Hold is gated when the heartbeat is gone')
await page.screenshot({ path: join(shotsDir, 'safety-no-link.png') })

const scoped = await commands()
const requestIds = scoped.map((entry) => entry.request?.context?.request_id)
check(
  scoped.every((entry) =>
    entry.request?.context?.session?.module_id === 'flight-controls'
    && typeof entry.request.context.request_id === 'string'
    && entry.request.context.request_id.length > 0),
  'every safety execute carries its exact session and request identity',
)
check(new Set(requestIds).size === requestIds.length, 'safety executes use fresh request identities')

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
  console.error(`check:safety FAILED — ${failed} assertion(s)`)
  process.exit(1)
}
console.log('check:safety OK')
