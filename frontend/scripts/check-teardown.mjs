// End-to-end check for the teardown guard (plan §4 L4, acceptance: "while
// armed, all three exit paths are refused; the typed confirmation is required
// when something would be lost").
//   node check-teardown.mjs <viteUrl> <shotsDir>
//
// The fake shell plays the two halves Rust owns: `teardown_guard` (the policy
// answer: refusal / keyword / forced) and the teardown commands themselves.
// The rules behind the guard are unit-tested in `core::lifecycle`
// (`TeardownGuard::of`); this script covers what the UI does with the answer —
// which is the part that used to not exist at all: the buttons used to fire
// straight at the shell.
import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node check-teardown.mjs <viteUrl> <shotsDir>')
  process.exit(1)
}

let failed = 0
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`)
  if (!ok) failed += 1
}

// Headless Chromium renders the Cesium map on a software GL stack, which
// starves the page's own timers, so waits are driven from Node.
async function waitUntil(fn, timeoutMs, stepMs = 250) {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    if (await fn()) return true
    if (Date.now() >= deadline) return false
    await page.waitForTimeout(stepMs)
  }
}

const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
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

  const snapshot = (atMs) => ({
    last_heartbeat_at_ms: atMs % 100000,
    heartbeat: {
      system_id: 1,
      component_id: 1,
      vehicle_type: 'QUADROTOR',
      autopilot: 'PX4',
      base_mode: {
        custom_mode_enabled: true,
        test_enabled: false,
        auto_enabled: false,
        guided_enabled: true,
        stabilize_enabled: true,
        hil_enabled: false,
        manual_input_enabled: false,
        safety_armed: false,
      },
      custom_mode: 4,
      mode_label: 'Hold',
      flight_state: 'OnGround',
      mavlink_version: 3,
    },
    global_position: {
      time_boot_ms: 1,
      latitude_deg: 49.2831,
      longitude_deg: -123.1207,
      altitude: { datum: 'AMSL', meters: 34 },
      relative_alt_m: 0,
      velocity: { x_m_s: 0, y_m_s: 0, z_m_s: 0 },
      heading_deg: 12,
    },
    attitude: { roll_deg: 0, pitch_deg: 0, yaw_deg: 12 },
    sys_status: { sensors: {}, battery_voltage_mv: 15400, battery_current_ma: 12000, battery_remaining_percent: 76 },
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

  /** What the fake shell answers for the next `teardown_guard`. */
  let guard = { refusal: null, keyword: null, forced: false, armed: false, on_ground: true }
  window.__mockCalls = []
  window.__mockGuardCalls = []
  window.__mockUnhandled = []
  window.__mockSetGuard = (next) => {
    guard = { ...guard, ...next }
  }
  window.__mockTelemetry = (atMs) => deliver('telemetry', snapshot(atMs))
  window.__mockLink = (status) => deliver('link', status)

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
      if (cmd === 'teardown_guard') {
        window.__mockGuardCalls.push({ reason: args.reason, atStake: args.atStake })
        const { keywordWhenAtStake, ...rest } = guard
        // Mirrors core's `TeardownGuard::of`: the keyword follows from what
        // the caller says is at stake, so the UI's own flag is exercised.
        return {
          reason: args.reason,
          ...rest,
          keyword: keywordWhenAtStake ? (args.atStake ? 'DISCONNECT' : null) : guard.keyword,
        }
      }
      // The teardown commands themselves: recorded, never run.
      if (cmd === 'disconnect' || cmd === 'shutdown_app') {
        window.__mockCalls.push(cmd)
        return null
      }
      window.__mockUnhandled.push(cmd)
      return null
    },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
})

await page.goto(url, { waitUntil: 'networkidle', timeout: 60000 })
await page.waitForTimeout(4000)
await mkdir(shotsDir, { recursive: true })

const dialog = page.locator('[data-testid="teardown-dialog"]')
const confirm = page.locator('[data-testid="teardown-confirm"]')
const calls = () => page.evaluate(() => window.__mockCalls)

// A connected, disarmed, on-ground vehicle: the state where exiting is legal.
await page.evaluate(() => {
  window.__mockTelemetry(Date.now())
  window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: true })
})
await page.waitForTimeout(500)

// --- 1. armed: refused, with the reason, from the link popover ---------------
await page.evaluate(() =>
  window.__mockSetGuard({ refusal: 'the vehicle is armed', keyword: null, armed: true }),
)
await page.getByTitle('Connection details').click()
await page.locator('[data-testid="link-disconnect"]').click()
await dialog.waitFor({ timeout: 5000 })
check(
  (await page.locator('[data-testid="teardown-refusal"]').innerText()).includes(
    'the vehicle is armed',
  ),
  'an armed vehicle is refused, with the shell\'s reason',
)
check((await confirm.count()) === 0, 'a refused teardown offers no confirm button')
check(
  (await page.evaluate(() => window.__mockCalls)).length === 0,
  'nothing was sent while refused',
)
await page.screenshot({ path: join(shotsDir, 'teardown-refused.png') })
await page.locator('[data-testid="teardown-cancel"]').click()
await dialog.waitFor({ state: 'detached', timeout: 5000 })
check((await dialog.count()) === 0, 'cancel closes the dialog')
check((await calls()).length === 0, 'cancel sends nothing')

// --- 2. the typed keyword gates disconnecting -------------------------------
await page.evaluate(() => window.__mockSetGuard({ refusal: null, keyword: 'DISCONNECT' }))
await page.getByTitle('Settings').click()
await page.locator('[data-testid="settings-disconnect"]').click()
await dialog.waitFor({ timeout: 5000 })
const keyword = page.locator('[data-testid="teardown-keyword"]')
check((await keyword.count()) === 1, 'the shell\'s keyword is asked for')
check(await confirm.isDisabled(), 'confirm is disabled until the keyword is typed')
await keyword.fill('disconnect')
await page.waitForTimeout(200)
check(await confirm.isDisabled(), 'the wrong (lowercase) keyword does not unlock it')
await page.screenshot({ path: join(shotsDir, 'teardown-keyword.png') })
await keyword.fill('DISCONNECT')
await page.waitForTimeout(200)
check(!(await confirm.isDisabled()), 'the exact keyword unlocks confirm')
await confirm.click()
await dialog.waitFor({ state: 'detached', timeout: 5000 })
check((await calls()).join(',') === 'disconnect', 'the confirmed keyword sends the disconnect')
const guardCalls = await page.evaluate(() => window.__mockGuardCalls)
check(
  guardCalls.some((c) => c.reason === 'disconnect' && c.atStake === false),
  `the guard was asked about disconnecting a clean plan (got ${JSON.stringify(guardCalls)})`,
)

// --- 3. quit: no keyword when nothing is at stake ---------------------------
await page.evaluate(() => window.__mockSetGuard({ refusal: null, keyword: null }))
// The Settings dialog from step 2 is still open (the teardown dialog is a
// second Radix dialog on top of it); only click the gear when it is not.
if ((await page.locator('[data-testid="settings-shutdown"]').count()) === 0) {
  await page.getByTitle('Settings').click()
}
await page.locator('[data-testid="settings-shutdown"]').click()
await dialog.waitFor({ timeout: 5000 })
check((await page.locator('[data-testid="teardown-keyword"]').count()) === 0, 'a clean quit asks for no keyword')
check(!(await confirm.isDisabled()), 'a clean quit can be confirmed')
await page.screenshot({ path: join(shotsDir, 'teardown-quit.png') })
await confirm.click()
await page.waitForTimeout(500)
check((await calls()).join(',') === 'disconnect,shutdown_app', 'the confirmed quit runs the shutdown')
const guardCalls2 = await page.evaluate(() => window.__mockGuardCalls)
check(
  guardCalls2.some((c) => c.reason === 'shutdown_app'),
  'the quit path asks the guard about shutdown_app',
)

// --- 4. unsaved plan work: the keyword is asked for, and named --------------
await page.evaluate(() =>
  window.__mockSetGuard({ refusal: null, keyword: null, keywordWhenAtStake: true }),
)
// Close the Settings dialog left open by step 3: the Plan tab is behind it.
await page.keyboard.press('Escape')
await page.waitForTimeout(300)
await page.getByRole('button', { name: 'Plan' }).click()
await page.getByRole('button', { name: '3 · Waypoints' }).click()
const coordInput = page.getByLabel('Add by coordinates')
await coordInput.fill('49.2831, -123.1207')
await coordInput.press('Enter')
await page.waitForTimeout(300)
check(
  await waitUntil(
    async () => (await page.locator('text=not on the flight controller').count()) === 0,
    1000,
    200,
  ),
  'a fresh plan has nothing at stake yet',
)

await page.getByTitle('Connection details').click()
await page.locator('[data-testid="link-disconnect"]').click()
await dialog.waitFor({ timeout: 5000 })
const atStake = page.locator('[data-testid="teardown-at-stake"]')
check(
  (await atStake.count()) === 1 && (await atStake.innerText()).includes('1 waypoint'),
  `the dialog names the work that would be lost (got "${await atStake.innerText()}")`,
)
check(
  (await page.locator('[data-testid="teardown-keyword"]').count()) === 1,
  'an unsaved plan requires the keyword',
)
const atStakeCalls = await page.evaluate(() => window.__mockGuardCalls)
check(
  atStakeCalls.some((c) => c.reason === 'disconnect' && c.atStake === true),
  `the guard is told what is at stake (got ${JSON.stringify(atStakeCalls)})`,
)
await page.screenshot({ path: join(shotsDir, 'teardown-at-stake.png') })
await page.locator('[data-testid="teardown-cancel"]').click()
await dialog.waitFor({ state: 'detached', timeout: 5000 })

const unhandled = await page.evaluate(() => window.__mockUnhandled)
if (unhandled.length > 0) console.log(`  unmocked commands (ignored): ${[...new Set(unhandled)].join(', ')}`)

await browser.close()

if (pageErrors.length > 0) {
  console.error('page errors:', pageErrors.slice(0, 8))
  process.exit(2)
}
if (failed > 0) {
  console.error(`check:teardown FAILED — ${failed} assertion(s)`)
  process.exit(1)
}
console.log('check:teardown OK')
