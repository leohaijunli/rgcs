// Logs deletion regression against the real React UI with an isolated IPC
// stub. No FC, shell, or file is touched. Usage:
//   node scripts/check-logs-delete.mjs [url] [screenshots]
// Verifies both destructive actions require two confirmations, that declining
// the first sends no command, and that the accepted path reaches the backend
// with the exact log identity.
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const require = createRequire(import.meta.url)
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright')
const url = process.argv[2] ?? 'http://localhost:5199/?nomap'
const shots = process.argv[3]
const browser = await chromium.launch({ headless: true })
const context = await browser.newContext({ viewport: { width: 1366, height: 768 } })
const page = await context.newPage()
const errors = []
page.on('pageerror', (error) => errors.push(String(error)))

let acceptDialogs = true
const dialogs = []
page.on('dialog', async (dialog) => {
  dialogs.push(dialog.message())
  if (acceptDialogs) await dialog.accept()
  else await dialog.dismiss()
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
        safety_armed: true,
      },
      custom_mode: (2 << 16) | (0 << 24),
      mode_label: 'Altitude',
      flight_state: 'active',
      mavlink_version: 3,
    },
    global_position: {
      time_boot_ms: 1,
      latitude_deg: 49.2831,
      longitude_deg: -123.1207,
      altitude: { datum: 'AMSL', meters: 61.5 },
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
  window.__mockTelemetry = (atMs) => deliver('telemetry', snapshot(atMs))
  window.__mockEmit = deliver
  window.__logsCalls = []
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
      if (cmd === 'link_status' || cmd === 'connect' || cmd === 'link_error') return null
      if (cmd === 'alert_snapshot' || cmd === 'enumerate_devices') return []
      if (cmd === 'flight_mode_table' || cmd === 'tools_status') return []
      if (cmd === 'logs_delete' || cmd === 'logs_erase_all') {
        window.__logsCalls.push({ cmd, args })
        return []
      }
      return null
    },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
})

const calls = () => page.evaluate(() => window.__logsCalls)
const emitList = async () => {
  for (let i = 0; i < 20; i += 1) {
    await page.evaluate(() =>
      window.__mockEmit('logs_event', {
        kind: 'list',
        entries: [
          { id: 1, time_utc: 1700000000, size: 2 * 1024 * 1024 },
          { id: 2, time_utc: 1700001000, size: 3 * 1024 * 1024 },
        ],
      }),
    )
    if ((await page.getByText('#1', { exact: true }).count()) > 0) return
    await page.waitForTimeout(100)
  }
  throw new Error('the log list did not render')
}

try {
  await page.goto(url, { waitUntil: 'networkidle', timeout: 60000 })
  await page.evaluate(() => {
    window.__mgProject?.setState({
      project: { version: 1, name: 'Mock', crs: 'EPSG:4326', datum: 'cgvd2013', areas: [], settings: null },
    })
    window.__mockTelemetry(Date.now())
    window.__mockEmit('link', { link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: true })
    setInterval(() => window.__mockTelemetry(Date.now()), 300)
  })

  await page.waitForSelector('[data-testid="autoconnect-dialog"]', { state: 'detached', timeout: 15000 }).catch(() => {})
  const targetConfirm = page.locator('[data-testid="target-change-confirm"]')
  await targetConfirm.waitFor({ state: 'visible', timeout: 8000 }).catch(() => {})
  if ((await targetConfirm.count()) > 0) await targetConfirm.click()
  await page.waitForSelector('[data-testid="target-change-dialog"]', { state: 'detached', timeout: 5000 }).catch(() => {})

  if (shots) await mkdir(shots, { recursive: true })

  await page.getByTestId('tools-open').click()
  await page.getByTestId('tool-logs').click()
  await page.getByRole('dialog').waitFor()
  await emitList()
  if (shots) await page.screenshot({ path: join(shots, 'logs-delete.png') })

  // Single delete accepts both confirmations and sends the exact identity.
  const row1 = page.locator('li', { hasText: '#1' }).getByRole('button', { name: 'Delete', exact: true })
  dialogs.length = 0
  await row1.click()
  await page.waitForTimeout(200)
  assert.equal(dialogs.length, 2, 'single delete asks twice')
  let recorded = await calls()
  assert.equal(recorded.length, 1, 'single delete reaches the backend once')
  assert.equal(recorded[0].cmd, 'logs_delete')
  assert.deepEqual(
    { id: recorded[0].args.id, size: recorded[0].args.size, timeUtc: recorded[0].args.timeUtc },
    { id: 1, size: 2 * 1024 * 1024, timeUtc: 1700000000 },
    'single delete carries id, size and time_utc',
  )

  // Declining the first confirmation sends nothing.
  await emitList()
  const row2 = page.locator('li', { hasText: '#2' }).getByRole('button', { name: 'Delete', exact: true })
  acceptDialogs = false
  dialogs.length = 0
  await row2.click()
  await page.waitForTimeout(200)
  acceptDialogs = true
  assert.equal(dialogs.length, 1, 'declining the first confirmation shows no second dialog')
  assert.equal((await calls()).length, 1, 'declining the first confirmation sends no command')

  // Delete all accepts both confirmations and sends erase-all.
  await emitList()
  dialogs.length = 0
  await page.getByRole('button', { name: 'Delete all…', exact: true }).click()
  await page.waitForTimeout(200)
  assert.equal(dialogs.length, 2, 'delete all asks twice')
  recorded = await calls()
  assert.equal(recorded.length, 2, 'delete all reaches the backend once')
  assert.equal(recorded[1].cmd, 'logs_erase_all')

  assert.deepEqual(errors, [])
  console.log('PASS: two-confirm delete all + single delete, decline sends nothing')
} finally {
  await browser.close()
}
