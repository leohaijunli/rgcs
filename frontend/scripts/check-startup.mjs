// End-to-end check for the L8 startup surface: the auto-connect 3 s cancel
// window, the flight-controller target-change confirmation, and the Prepare
// stage self-check rows.
//
//   node scripts/check-startup.mjs <viteUrl> <shotsDir>
//
// The mock shell intercepts `connect`, `config_load/save` and the
// calibration/clock commands; `window.__mockSetTarget` changes the heartbeat
// sysid so a "different aircraft" can be injected.

import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node scripts/check-startup.mjs <viteUrl> <shotsDir>')
  process.exit(1)
}

let failed = 0
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`)
  if (!ok) failed += 1
}

const browser = await chromium.launch({
  headless: true,
  args: ['--disable-gpu', '--disable-dev-shm-usage'],
})

/** One page run: fresh load, auto-connect pending, mock wiring in place. */
async function boot(tick) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
  await page.addInitScript(
    ({ tick }) => {
      const commands = []
      let sysid = tick.sysid
      const handlers = {}
      const deliver = (event, payload) => {
        const cb = window[`__mock_cb_${handlers[event]}`]
        if (typeof cb === 'function') cb({ event, payload })
      }
      const at = Date.now()
      window.__mockTelemetry = () =>
        deliver('telemetry', {
          last_heartbeat_at_ms: at,
          heartbeat: {
            system_id: sysid,
            component_id: 1,
            vehicle_type: tick.vehicle,
            autopilot: 'px4',
            base_mode: { custom_mode_enabled: true },
            custom_mode: 0,
            mode_label: 'Hold',
            flight_state: 'active',
            mavlink_version: 3,
          },
          global_position: null,
          attitude: { roll_deg: 0, pitch_deg: 0, yaw_deg: 0 },
          sys_status: null,
          battery: null,
          gps: null,
          field_ages: { heartbeat_at_ms: at },
          extended_sys_state: null,
          home: null,
          estimator: null,
          vibration: null,
          rc_channels: null,
          radio: null,
        })
      window.__mockSetTarget = (next) => {
        sysid = next
        window.__mockTelemetry()
      }
      window.__TAURI_INTERNALS__ = {
        transformCallback: (cb) => {
          const id = Math.floor(Math.random() * 1e9)
          window[`__mock_cb_${id}`] = cb
          return id
        },
        invoke: async (cmd, args) => {
          if (cmd === 'plugin:event|listen') {
            handlers[args.event] = args.handler
            return 0
          }
          if (cmd.startsWith('plugin:event|')) return null
          if (cmd === 'link_status') return null
          if (cmd === 'config_load') return null
          if (cmd === 'mag_calibration_status') return { loaded: true, hash8: 'ab12cd34' }
          if (cmd === 'clock_info') return { epoch_ms: Date.now(), timezone: 'America/Vancouver' }
          if (cmd === 'udev_rules_present') return true
          if (cmd === 'flight_mode_table') return []
          if (cmd === 'alert_snapshot') return []
          if (cmd === 'enumerate_devices') return []
          if (cmd === 'connect') commands.push({ cmd, endpoint: args?.endpoint })
          else commands.push({ cmd, ...(args ?? {}) })
          return null
        },
      }
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
      window.__mockCommands = commands
      window.__mockLinkEvent = (status) => deliver('link', status)
      // Open a throwaway project the moment the store handle exists (the
      // forced start page would otherwise cover the surfaces under test).
      const iv = setInterval(() => {
        if (window.__mgProject) {
          clearInterval(iv)
          window.__mgProject.setState({
            project: { version: 1, name: 'Mock', crs: 'EPSG:4326', datum: 'cgvd2013', areas: [], settings: null },
          })
        }
      }, 10)
    },
    { tick },
  )
  await page.goto(`${url}` + (url.includes('?') ? '&' : '?') + 'nomap', {
    waitUntil: 'networkidle',
    timeout: 60000,
  })
  await mkdir(shotsDir, { recursive: true })
  return page
}

const dialog = (page) => page.locator('[data-testid="autoconnect-dialog"]')
const targetDialog = (page) => page.locator('[data-testid="target-change-dialog"]')

// --- 1. the boot connect: the 3 s window expires and fires -----------------
{
  const page = await boot({ sysid: 1, vehicle: 'quadrotor' })
  check(
    await page
      .waitForFunction(() => window.__mockCommands.some((c) => c.cmd === 'connect' && c.endpoint === 'udpin:0.0.0.0:14550'), { timeout: 15000 })
      .then(() => true)
      .catch(() => false),
    'expiry connects to the configured endpoint after the 3 s window',
  )
  await page.close()
}

// --- 2. the cancel window: dialog shows, cancel stops the connect ----------
{
  const page = await boot({ sysid: 1, vehicle: 'quadrotor' })
  await page.waitForFunction(() => window.__mockCommands.some((c) => c.cmd === 'connect'), { timeout: 15000 }).catch(() => {})
  const connectsAtStart = await page.evaluate(() => window.__mockCommands.filter((c) => c.cmd === 'connect').length)
  await page.evaluate(() => window.__mgAutoConnect.getState().request('udpin:0.0.0.0:14550'))
  check(await page.waitForSelector('[data-testid="autoconnect-dialog"]', { timeout: 5000 }).then(() => true).catch(() => false), 'the auto-connect dialog shows the pending endpoint')
  const body = await page.locator('[data-testid="autoconnect-dialog"]').innerText()
  check(/udpin:0\.0\.0\.0:14550/.test(body), 'the dialog names the endpoint it would connect to')
  await page.screenshot({ path: join(shotsDir, 'startup-autoconnect.png') })
  await page.locator('[data-testid="autoconnect-cancel"]').click()
  await page.waitForTimeout(4000)
  const connects = await page.evaluate(() => window.__mockCommands.filter((c) => c.cmd === 'connect').length)
  check(connects === connectsAtStart, 'cancel leaves no extra connect fired')

    // --- 3. a re-requested window expires and connects again -----------------
  await page.evaluate(() => window.__mgAutoConnect.getState().request('udpin:0.0.0.0:14550'))
  check(
    await page
      .waitForFunction((n) => window.__mockCommands.filter((c) => c.cmd === 'connect').length > n, connectsAtStart, { timeout: 8000 })
      .then(() => true)
      .catch(() => false),
    'a re-requested window expires and connects',
  )

  // --- 4. first connect: the target-change dialog asks for confirmation ---
  await page.evaluate(() => window.__mockLinkEvent({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: true }))
  await page.evaluate(() => window.__mockTelemetry())
  check(
    await page.waitForSelector('[data-testid="target-change-dialog"]', { timeout: 8000 }).then(() => true).catch(() => false),
    'a first connect asks the operator to confirm the aircraft',
  )
  await page.screenshot({ path: join(shotsDir, 'startup-target-change.png') })
  await page.locator('[data-testid="target-change-confirm"]').click()
  check(
    await page
      .waitForFunction(() => {
        const saves = window.__mockCommands.filter((c) => c.cmd === 'config_save')
        return saves.some((c) => c.config?.last_target?.system_id === 1 && c.config?.last_target?.vehicle === 'quadrotor')
      }, { timeout: 5000 })
      .then(() => true)
      .catch(() => false),
    'confirming persists the target into the config',
  )
  check((await targetDialog(page).count()) === 0, 'the dialog closes after confirming')

  // --- 5. a different sysid re-triggers the check --------------------------
  await page.evaluate(() => window.__mockSetTarget(7))
  check(
    await page.waitForSelector('[data-testid="target-change-dialog"]', { timeout: 8000 }).then(() => true).catch(() => false),
    'a different sysid re-opens the confirmation',
  )
  await page.locator('[data-testid="target-change-disconnect"]').click()
  check(
    await page.waitForFunction(() => window.__mockCommands.some((c) => c.cmd === 'disconnect'), { timeout: 5000 }).then(() => true).catch(() => false),
    'refusing disconnects the link',
  )

  // --- 6. the Prepare stage carries the self-check rows --------------------
  await page.locator('[data-testid="workflow-open"]').click()
  await page.locator('[data-testid="workflow-stage-prepare"]').click()
  check(await page.waitForSelector('[data-testid="prepare-panel"]', { timeout: 8000 }).then(() => true).catch(() => false), 'the Prepare panel renders')
  const panel = await page.locator('[data-testid="prepare-panel"]').innerText()
  check(/ab12cd34/.test(panel), 'the calibration row shows the sha256 prefix')
  check(/America\/Vancouver/.test(panel), 'the clock row shows the timezone')
  check(/udev rules installed/.test(panel), 'the serial row reports the udev rules')
  await page.screenshot({ path: join(shotsDir, 'startup-prepare-rows.png') })
  await page.close()
}

await browser.close()
if (failed > 0) {
  console.log(`${failed} FAILED`)
  process.exit(1)
}
console.log('check-startup OK')
