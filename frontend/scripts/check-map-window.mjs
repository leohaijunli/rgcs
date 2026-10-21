// U15 main-window acceptance: with the map detached (its own webview), the
// main window must stay self-sufficient — the stage bar, HUD, command strip
// and alerts render, RTL stays reachable in ≤2 interactions, and a "bring
// map back" affordance is present. The detached MAP window itself needs the
// Tauri shell (manual/field check); this script covers the main-window side
// with a mocked `map_status`.
//
//   node scripts/check-map-window.mjs <viteUrl> <shotsDir>

import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node scripts/check-map-window.mjs <viteUrl> <shotsDir>')
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

async function boot(detached) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
  await page.addInitScript(
    ({ detached }) => {
      const handlers = {}
      const deliver = (event, payload) => {
        const cb = window[`__mock_cb_${handlers[event]}`]
        if (typeof cb === 'function') cb({ event, payload })
      }
      const at = Date.now()
      window.__mockLink = (s) => deliver('link', s)
      window.__mockTelemetry = () =>
        deliver('telemetry', {
          last_heartbeat_at_ms: at,
          heartbeat: { system_id: 1, component_id: 1, vehicle_type: 'quadrotor', autopilot: 'px4', base_mode: {}, custom_mode: 0, mode_label: 'Hold', flight_state: 'active', mavlink_version: 3 },
          global_position: { time_boot_ms: 1, latitude_deg: 48.5, longitude_deg: -123.4, altitude: { datum: 'AMSL', meters: 60 }, relative_alt_m: 40, velocity: { x_m_s: 0, y_m_s: 0, z_m_s: 0 }, heading_deg: 0 },
          attitude: { roll_deg: 0, pitch_deg: 0, yaw_deg: 0 },
          sys_status: { sensors: {}, battery_voltage_mv: 15400, battery_current_ma: 0, battery_remaining_percent: 80 },
          battery: null,
          gps: { fix_type: 'RTK_FIXED', satellites_visible: 18 },
          field_ages: { heartbeat_at_ms: at, global_position_at_ms: at },
          extended_sys_state: { landed_state: 'in_air', vtol_state: 'undefined' },
          home: null,
          estimator: null,
          vibration: null,
          rc_channels: null,
          radio: null,
        })
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
          if (cmd === 'map_status') return detached
          if (cmd === 'map_close') return true
          if (['enumerate_devices', 'flight_mode_table', 'alert_snapshot'].includes(cmd)) return []
          return null
        },
      }
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
      const iv = setInterval(() => {
        if (window.__mgProject) {
          clearInterval(iv)
          window.__mgProject.setState({
            project: { version: 1, name: 'Mock', crs: 'EPSG:4326', datum: 'cgvd2013', areas: [], settings: null },
          })
        }
      }, 10)
    },
    { detached },
  )
  await page.goto(`${url}` + (url.includes('?') ? '&' : '?') + 'nomap', {
    waitUntil: 'networkidle',
    timeout: 60000,
  })
  await page.evaluate(() => {
    window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: true })
    window.__mockTelemetry()
    setInterval(() => window.__mockTelemetry(), 300)
  })
  // L8 startup dialogs: wait out the auto-connect window, confirm the target.
  await page
    .waitForSelector('[data-testid="autoconnect-dialog"]', { state: 'detached', timeout: 15000 })
    .catch(() => {})
  const targetConfirm = page.locator('[data-testid="target-change-confirm"]')
  await targetConfirm.waitFor({ state: 'visible', timeout: 8000 }).catch(() => {})
  if ((await targetConfirm.count()) > 0) await targetConfirm.click()
  await page.waitForTimeout(300)
  await mkdir(shotsDir, { recursive: true })
  return page
}

// --- 1. detached: the main window keeps every control ----------------------
{
  const page = await boot(true)
  check(
    await page
      .waitForSelector('[data-testid="map-detached-placeholder"]', { timeout: 8000 })
      .then(() => true)
      .catch(() => false),
    'the main window shows the detached-map placeholder',
  )
  check(
    await page.getByRole('button', { name: 'Bring map back' }).count() === 1,
    'a bring-back affordance is present',
  )
  // Self-sufficiency: stage bar, RTL/Hold pair, alerts and the command strip.
  check((await page.locator('[data-testid="flight-menu"]').count()) === 1, 'the Flight menu stays available')
  check((await page.getByRole('button', { name: 'RTL' }).count()) === 1, 'RTL stays reachable without the map')
  check(
    (await page.getByRole('button', { name: 'Prepare', exact: true }).count()) === 1,
    'the stage navigation stays available',
  )
  check((await page.locator('[data-testid="alert-open"]').count()) === 1, 'the alert center stays available')
  await page.screenshot({ path: join(shotsDir, 'map-detached-main.png') })
  await page.close()
}

// --- 2. not detached: no "detached" wording, no bring-back button ----------
// (`?nomap` still suppresses MapView in this harness, so the placeholder may
// render — what matters is that it is NOT the detached variant.)
{
  const page = await boot(false)
  const text = await page
    .locator('[data-testid="map-detached-placeholder"]')
    .innerText()
    .catch(() => '')
  check(!/own window/i.test(text), 'without detaching the placeholder is not the detached variant')
  check(
    (await page.getByRole('button', { name: 'Bring map back' }).count()) === 0,
    'no bring-back button while the map is not detached',
  )
  await page.close()
}

await browser.close()
if (failed > 0) {
  console.log(`${failed} FAILED`)
  process.exit(1)
}
console.log('check-map-window OK')
