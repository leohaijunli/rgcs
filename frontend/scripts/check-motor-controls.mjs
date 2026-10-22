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
const page = await browser.newPage({ viewport: { width: 1366, height: 768 } })
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
    output_values: [], fc_output_pwm: [], pwm_models: [],
    output_map: { motors: [], state: 'unknown', issues: [] },
    sd_capture: capture, run: { status: null, manifest_path: null },
  }
  window.__bench = {
    calls: [], dialogs: [], status, downloadResult: false,
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
        status.session = 'running'
        test.emit('actuator_event', { kind: 'state', state: 'manual_running' })
        return
      }
      if (command === 'actuator_stop') {
        status.session = 'idle'
        test.emit('actuator_event', { kind: 'stopped', reason: 'command' })
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
    await start().click()
    await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'START')
    assert.equal(await calls('actuator_stop'), 1)
  }

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

  if (shots) {
    await mkdir(shots, { recursive: true })
    await page.screenshot({ path: join(shots, 'motor-controls-fixed-1366x768.png') })
  }
  assert.deepEqual(errors, [])
  console.log('PASS: quick clicks, 3-second hold, release-click suppression, independent capture, save cancellation, and verified delete-list feedback')
} finally {
  await browser.close()
}
