// Regression checks against the real React UI with an isolated IPC stub.
// No hardware connection or motor command is made by this test.
// Usage: PLAYWRIGHT_MODULE=/path/to/playwright node scripts/check-motor-controls.mjs [url] [screenshots]
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const require = createRequire(import.meta.url)
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright')
const url = process.argv[2] ?? 'http://127.0.0.1:1420/actuator.html'
const shots = process.argv[3]
const browser = await chromium.launch({ headless: true })
const context = await browser.newContext({ viewport: { width: 1366, height: 768 } })
const page = await context.newPage()
const errors = []
const dialogs = []
page.on('pageerror', (error) => errors.push(String(error)))
page.on('dialog', async (dialog) => {
  dialogs.push(dialog.message())
  await dialog.accept()
})
await page.addInitScript(() => {
  const callbacks = new Map()
  const listeners = new Map()
  let callbackId = 0
  const capture = {
    phase: 'idle', test_id: null, remote_path: null, logger: null,
    sdlog_mode: 0, sdlog_backend: 1, last_error: null,
  }
  const status = {
    connected: false, fc_alive: false, target_fc_alive: true, armed: false,
    custom_mode: 0, autopilot_px4: true, mode_label: 'Manual', endpoint: null,
    tick_hz: 10, session: 'idle', sitl_connected: false, sitl_endpoint: null,
    sitl_source: null, sitl_feed_age_s: null, target_connected: true,
    target_endpoint: 'isolated IPC test', target_system_id: 250,
    com_mot_test_en: 1, sdlog_mode: 0, sdlog_backend: 1,
    parameter_restart_pending: false, input_values: [], input_pwm: [],
    output_values: [],
    fc_output_pwm: [1500, 1503, 1740, null],
    input_values: [0.4, 0.5, 0.6, 0.7],
    input_pwm: [1400, 1500, 1600, 1700],
    pwm_models: [
      { min_us: 1000, max_us: 2000, reversed: false, thrust_factor: 0, disabled: false },
      { min_us: 1000, max_us: 2000, reversed: false, thrust_factor: 0, disabled: false },
      { min_us: 1000, max_us: 2000, reversed: true, thrust_factor: 0, disabled: false },
      { min_us: 1000, max_us: 2000, reversed: false, thrust_factor: 0, disabled: false },
    ],
    output_map: {
      motors: [
        { motor: 1, channel: { port: 'main', channel: 1 } },
        { motor: 2, channel: { port: 'main', channel: 2 } },
        { motor: 3, channel: { port: 'main', channel: 3 } },
        { motor: 4, channel: { port: 'aux', channel: 1 } },
      ],
      state: 'complete',
      issues: [],
    },
    sd_capture: capture, run: { status: null, manifest_path: null },
  }
  window.__bench = {
    calls: [], dialogs: [], status, downloadResult: false,
    startError: null, mappingError: null,
    entries: [{ id: 0, size: 1024, time_utc: 1700000000 }],
    emit(event, payload) {
      for (const [id, listener] of listeners) {
        if (listener.event === event) callbacks.get(listener.handler)?.({ id, event, payload })
      }
    },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: (_event, id) => listeners.delete(id),
  }
  window.__TAURI_INTERNALS__ = {
    transformCallback(callback) { callbacks.set(++callbackId, callback); return callbackId },
    unregisterCallback(id) { callbacks.delete(id) },
    async invoke(command, args) {
      const test = window.__bench
      test.calls.push({ command, args })
      if (command === 'plugin:event|listen') {
        listeners.set(args.handler, args)
        return args.handler
      }
      if (command === 'plugin:event|unlisten') { listeners.delete(args.eventId); return }
      if (command === 'actuator_status') return structuredClone(status)
      if (command === 'actuator_logs_status') return { entries: structuredClone(test.entries), error: null }
      if (command === 'actuator_logs_request_list') {
        test.emit('actuator_logs', { kind: 'list', entries: structuredClone(test.entries) })
        return structuredClone(test.entries)
      }
      if (command === 'actuator_logs_download') {
        if (test.downloadResult) {
          test.emit('actuator_logs', { kind: 'list', entries: structuredClone(test.entries) })
          setTimeout(() => test.emit('actuator_logs', { kind: 'downloaded', id: args.id, path: 'verified.ulg' }), 50)
        }
        return test.downloadResult
      }
      if (command === 'actuator_logs_delete' || command === 'actuator_logs_erase_all') {
        test.entries = []
        test.emit('actuator_logs', { kind: 'list', entries: [] })
        return []
      }
      if (command.startsWith('actuator_start_')) {
        if (test.startError) throw new Error(test.startError)
        status.session = 'running'
        status.output_values = [0.5, 0.5, 0.25, 0]
        test.emit('actuator_event', { kind: 'state', state: 'manual_running' })
        return
      }
      if (command === 'actuator_stop') {
        status.session = 'idle'
        status.output_values = []
        test.emit('actuator_event', { kind: 'stopped', reason: 'command' })
        return
      }
      if (command === 'actuator_refresh_mapping') {
        if (test.mappingError) throw new Error(test.mappingError)
        return
      }
      if (command === 'actuator_capture_start') {
        capture.phase = 'recording'
        capture.remote_path = '/fs/microsd/log/2026-10-22/test.ulg'
        return structuredClone(capture)
      }
      if (command === 'actuator_capture_stop') {
        capture.phase = 'complete'
        return structuredClone(capture)
      }
      if (command === 'params_cache_get' || command === 'config_load') return null
      return undefined
    },
  }
})

const calls = async (command) => page.evaluate((name) => window.__bench.calls.filter((call) => call.command === name).length, command)
const resetCalls = () => page.evaluate(() => { window.__bench.calls = [] })
const start = () => page.locator('button.start-btn')

try {
  await page.goto(url, { waitUntil: 'networkidle' })
  assert.equal(await page.locator('.actuator-left button.start-btn').count(), 1, 'one start control in the left sidebar')
  assert.equal(await page.locator('.actuator-left [data-testid="motor-mag-alarm"]').count(), 1, 'magnetic alarm shares the left sidebar')
  assert.equal(await page.getByTestId('motor-session-state').textContent(), 'LOCKED', 'one explicit locked state in the header')
  assert.equal(await page.locator('.state-strip').count(), 0, 'no contradictory duplicate state strip')
  const sourceDetails = page.getByRole('button', { name: 'SITL source details', exact: true })
  assert.equal(await sourceDetails.getAttribute('aria-expanded'), 'false', 'inactive SITL configuration starts collapsed')
  await page.getByRole('button', { name: 'Forward', exact: true }).click()
  await page.waitForFunction(() => document.getElementById('motor-sitl-details')?.hidden === false)
  await page.getByRole('button', { name: 'Manual', exact: true }).click()
  await page.waitForFunction(() => document.getElementById('motor-sitl-details')?.hidden === true)
  assert.equal(await calls('actuator_start_forward'), 0, 'changing the displayed source never starts a test')
  await page.getByRole('button', { name: 'Unlock test', exact: true }).click()
  for (const label of ['No props', 'Flat plate']) {
    if (label === 'Flat plate') {
      await page.getByRole('button', { name: label, exact: true }).click()
      await page.getByRole('button', { name: 'Unlock test', exact: true }).click()
    }
    await resetCalls()
    await start().click({ delay: 20 })
    await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'STOP')
    await page.waitForTimeout(1200)
    assert.equal(await calls('actuator_start_manual'), 1, `${label}: one short click starts once`)
    assert.equal(await calls('actuator_stop'), 0, `${label}: release never clicks STOP`)
    assert.equal(await start().textContent(), 'STOP')
    if (label === 'No props') {
      await page.getByText('tracking', { exact: true }).first().waitFor()
      const rows = page.locator('.motor-table tbody tr')
      assert.equal(await rows.nth(0).locator('td').nth(3).textContent(), '1500 µs', 'M1 expected pulse from the sent command')
      assert.equal(await rows.nth(0).locator('td').nth(5).textContent(), '0 µs', 'M1 tracks')
      assert.equal(await rows.nth(2).locator('td').nth(3).textContent(), '1750 µs', 'M3 inverted model expected pulse')
      assert.equal(await rows.nth(2).locator('td').nth(6).textContent(), 'mismatch', 'M3 outside ±5 µs')
      assert.equal(await rows.nth(3).locator('td').nth(6).textContent(), 'no feedback', 'M4 has no reported pulse')
    }
    if (label === 'Flat plate') {
      await page.locator('.actuator-header .stop-tab').click()
    } else {
      await start().click()
    }
    await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'START')
    assert.equal(await calls('actuator_stop'), 1)
  }

  await page.evaluate(() => { window.__bench.mappingError = 'simulated mapping failure' })
  await page.getByRole('button', { name: 'Refresh mapping', exact: true }).click()
  await page.getByText(/simulated mapping failure/).waitFor()
  await page.getByRole('button', { name: 'Dismiss', exact: true }).click()
  assert.equal(await page.getByText(/simulated mapping failure/).count(), 0, 'mapping error dismisses')
  await page.evaluate(() => { window.__bench.mappingError = null })
  await page.getByRole('button', { name: 'Unlock test', exact: true }).click()

  await page.evaluate(() => { window.__bench.startError = 'simulated start rejection' })
  await resetCalls()
  await start().click()
  await page.getByText(/start failed: Error: simulated start rejection/).waitFor()
  assert.equal(await calls('actuator_start_manual'), 1)
  await page.getByRole('button', { name: 'Dismiss', exact: true }).click()
  await page.evaluate(() => { window.__bench.startError = null })

  await page.getByRole('button', { name: 'Preset', exact: true }).click()
  await page.getByRole('button', { name: 'Unlock test', exact: true }).click()
  await page.getByRole('button', { name: 'Sine', exact: true }).click()
  await page.getByRole('button', { name: 'Unlock test', exact: true }).waitFor()
  await page.getByRole('button', { name: 'Manual', exact: true }).click()

  await page.getByRole('button', { name: 'Full props', exact: true }).click()
  await page.getByRole('button', { name: 'Unlock test', exact: true }).click()
  await page.getByPlaceholder('START PROPS').fill('START PROPS')
  await resetCalls()
  await start().click({ delay: 100 })
  assert.equal(await calls('actuator_start_manual'), 0, 'short props hold is canceled')
  await start().scrollIntoViewIfNeeded()
  const bounds = await start().boundingBox()
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)
  await page.mouse.down()
  await page.waitForTimeout(3150)
  assert.equal(await calls('actuator_start_manual'), 1, 'full props requires 3 s')
  await page.mouse.up()
  await page.waitForTimeout(200)
  assert.equal(await calls('actuator_stop'), 0, 'props hold release does not trigger STOP')
  await start().click()
  await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'START')

  await page.getByRole('button', { name: 'No props', exact: true }).click()
  await page.getByRole('button', { name: 'Unlock test', exact: true }).click()
  await resetCalls()
  await page.getByRole('button', { name: 'Start capture', exact: true }).click()
  await start().click()
  await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'STOP')
  await start().click()
  await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'START')
  assert.equal(await calls('actuator_capture_stop'), 0, 'motor STOP does not stop manual capture')
  await page.getByRole('button', { name: 'Stop capture', exact: true }).click()

  await page.getByRole('button', { name: 'Log retrieval', exact: true }).click()
  await page.getByRole('button', { name: 'Refresh list', exact: true }).click()
  await page.getByRole('button', { name: 'Download', exact: true }).click()
  await page.waitForTimeout(100)
  assert.equal(await page.getByRole('button', { name: 'Refresh list', exact: true }).isEnabled(), true, 'canceling save restores controls')
  await page.evaluate(() => { window.__bench.downloadResult = true })
  await page.getByRole('button', { name: 'Download', exact: true }).click()
  await page.getByText('saved', { exact: true }).waitFor()
  assert.equal(await page.getByRole('button', { name: 'Delete', exact: true }).isEnabled(), true, 'downloaded logs can be deleted')
  await page.getByRole('button', { name: 'Delete', exact: true }).click()
  await page.getByText('No SD logs on the target.', { exact: true }).waitFor()
  assert.equal(dialogs.length, 2, 'per-log delete requires both confirmations')
  assert.equal(await page.locator('.log-list li').count(), 1, 'deleted entry disappears')
  await page.evaluate(() => { window.__bench.entries = [{ id: 0, size: 1024, time_utc: 1700000000 }] })
  await page.getByRole('button', { name: 'Refresh list', exact: true }).click()
  await page.getByRole('button', { name: 'Delete all…', exact: true }).click()
  await page.getByText('No SD logs on the target.', { exact: true }).waitFor()
  assert.equal(dialogs.length, 4, 'delete-all requires both confirmations')

  const alarm = page.getByTestId('motor-mag-alarm')
  const startsBeforeAlarm = await calls('actuator_start_manual')
  await alarm.getByRole('checkbox', { name: 'Enabled', exact: true }).check()
  await page.waitForFunction(() => document.querySelector('[data-testid="motor-mag-state"]')?.textContent === 'No data')
  await page.evaluate(() => window.__bench.emit('actuator_telemetry', { mag_total_nt: 60000, alarm: 'alarm' }))
  await page.waitForFunction(() => document.querySelector('[data-testid="motor-mag-state"]')?.textContent === 'ALARM')
  await alarm.getByRole('button', { name: 'Acknowledge (silence)', exact: true }).click()
  assert.equal(await calls('actuator_acknowledge_alarm'), 1, 'left-sidebar acknowledgement reaches the existing alarm handler')
  await alarm.getByRole('checkbox', { name: 'Enabled', exact: true }).uncheck()
  await page.waitForFunction(() => document.querySelector('[data-testid="motor-mag-state"]')?.textContent === 'Disabled')
  assert.equal(await calls('actuator_start_manual'), startsBeforeAlarm, 'alarm controls never issue motor starts')

  // Explicitly labelled, deterministic traces exercise all three plot panes.
  await page.evaluate(() => {
    const test = window.__bench
    test.emit('actuator_event', { kind: 'stopped', reason: 'simulated IPC fixture' })
    const now = Date.now() / 1000
    const samples = []
    for (let i = 0; i <= 200; i++) {
      const t = now - 20 + i / 10
      samples.push(
        { id: 'fc.servo1', t, v: 1500 + 20 * Math.sin(i / 10) },
        { id: 'mag.raw_total', t, v: 50000 + 50 * Math.sin(i / 8) },
        { id: 'esc1.rpm', t, v: 4000 + 300 * Math.sin(i / 12) },
      )
    }
    test.emit('actuator_samples', samples)
  })
  await page.waitForFunction(() => document.querySelectorAll('.scope-pane').length === 3)
  await page.getByRole('button', { name: 'Log retrieval', exact: true }).click()

  const signalList = page.getByRole('button', { name: 'Signal list', exact: true })
  await signalList.click()
  await page.locator('#motor-signal-list').waitFor({ state: 'hidden' })
  assert.equal(await page.locator('#motor-signal-list').isVisible(), false, 'signal list collapses on a wide window')
  await signalList.click()
  await page.locator('#motor-signal-list').waitFor({ state: 'visible' })

  if (shots) await mkdir(shots, { recursive: true })
  for (const [width, height] of [[1024, 640], [1366, 768], [1920, 1080]]) {
    await page.setViewportSize({ width, height })
    await page.waitForTimeout(150)
    await page.locator('.scope-pane').last().scrollIntoViewIfNeeded()
    const lastPlot = await page.locator('.scope-pane-title').last().boundingBox()
    assert(lastPlot && lastPlot.y >= 0 && lastPlot.y + lastPlot.height <= height, 'last plot remains reachable')
    await page.getByRole('button', { name: 'Start capture', exact: true }).scrollIntoViewIfNeeded()
    const stop = await page.locator('.actuator-header .estop-btn').boundingBox()
    assert(stop && stop.y >= 0 && stop.y + stop.height <= height && stop.x + stop.width <= width, 'emergency stop stays visible while another pane scrolls')
    await page.locator('.actuator-grid').evaluate((root) => {
      root.scrollTop = 0
      for (const pane of root.querySelectorAll('.actuator-left,.actuator-mid,.actuator-right,.scope-panes')) pane.scrollTop = 0
    })
    const layout = await page.locator('.actuator-grid').evaluate((root) => ({
      width: root.clientWidth,
      scrollWidth: root.scrollWidth,
      height: root.clientHeight,
      scrollHeight: root.scrollHeight,
      startTop: root.querySelector('.start-btn').getBoundingClientRect().top,
      midTop: root.querySelector('.actuator-mid').getBoundingClientRect().top,
      paneHeights: [...root.querySelectorAll('.actuator-left,.actuator-mid,.actuator-right')].map((pane) => pane.clientHeight),
    }))
    assert(layout.scrollWidth <= layout.width + 1, 'the workspace does not overflow horizontally')
    if (width > 1100) {
      assert(layout.scrollHeight <= layout.height + 1, 'wide workspaces cannot grow past the viewport')
      assert(layout.paneHeights.every((h) => h < height), 'each wide pane has a bounded height')
      const startBox = await start().boundingBox()
      assert(startBox && startBox.y >= 0 && startBox.y + startBox.height <= height, 'sidebar start is visible without scrolling on desktop')
    } else {
      assert(layout.startTop < layout.midTop, 'stacked start controls precede the plotting workspace')
    }
    console.log(`PASS: ${width}x${height} bounded layout, reachable plots/capture, fixed stop controls`)
    if (shots) await page.screenshot({ path: join(shots, `motor-controls-${width}x${height}.png`) })
  }

  // A second same-origin page models the main window changing preferences.
  const preferences = await page.context().newPage()
  await preferences.goto(url, { waitUntil: 'networkidle' })
  const startsBeforePreferences = await calls('actuator_start_manual')
  const stopsBeforePreferences = await calls('actuator_stop')
  await preferences.evaluate(() => {
    localStorage.setItem('maggcs.theme', 'light')
    localStorage.setItem('maggcs.fieldMode', 'true')
  })
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light' && document.documentElement.classList.contains('field-mode'))
  await page.setViewportSize({ width: 1366, height: 768 })
  await page.locator('.actuator-grid').evaluate((root) => {
    root.scrollTop = 0
    for (const pane of root.querySelectorAll('.actuator-left,.actuator-mid,.actuator-right,.scope-panes')) pane.scrollTop = 0
  })
  const undersized = await page.locator('.actuator-grid').evaluate((root) =>
    [...root.querySelectorAll('button,select,input:not([type="checkbox"]),summary,label:has(input[type="checkbox"])')]
      .filter((el) => el.getClientRects().length > 0)
      .filter((el) => el.getBoundingClientRect().height < 44 || el.getBoundingClientRect().width < 44)
      .map((el) => el.textContent || el.getAttribute('aria-label')),
  )
  assert.deepEqual(undersized, [], 'field-mode interactive targets are at least 44 px')
  assert.equal(await calls('actuator_start_manual'), startsBeforePreferences, 'preference updates do not start motors')
  assert.equal(await calls('actuator_stop'), stopsBeforePreferences, 'preference updates do not stop motors')
  if (shots) await page.screenshot({ path: join(shots, 'motor-controls-field-light.png') })
  await preferences.close()
  await page.reload({ waitUntil: 'networkidle' })
  assert.equal(await page.evaluate(() => document.documentElement.classList.contains('field-mode')), true, 'field mode is restored at window boot')
  assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), 'light', 'theme is restored at window boot')
  assert.deepEqual(errors, [])
  console.log('PASS: start/stop gestures, tracking, persistent errors, capture/log controls, sidebar alarm, three viewport layouts and display preference synchronization')
} finally {
  await browser.close()
}
