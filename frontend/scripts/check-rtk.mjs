// Real drawer/store with isolated IPC fixtures. No serial device is opened.
// node scripts/check-rtk.mjs [vite URL] [screenshots directory]
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const require = createRequire(import.meta.url)
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright')
const url = process.argv[2] ?? 'http://localhost:5199/'
const shots = process.argv[3]
const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } })
const errors = []
page.on('pageerror', (error) => errors.push(String(error)))

async function until(check, label) {
  const deadline = Date.now() + 6000
  while (!await check()) {
    assert.ok(Date.now() < deadline, label)
    await page.waitForTimeout(30)
  }
}

async function shot(name) { if (shots) await page.screenshot({ path: join(shots, name) }) }

try {
  if (shots) await mkdir(shots, { recursive: true })
  await page.addInitScript(() => {
    localStorage.setItem('maggcs.autoconnect', 'false')
    const callbacks = new Map()
    const listeners = new Map()
    let next = 0
    const device = (port, vendor, product, name) => ({
      port_name: port, transport: 'usb', device_id: { vendor_id: vendor, product_id: product },
      serial_number: 'receiver-A', manufacturer: null, product: name,
    })
    const fixture = {
      calls: [], stopError: null, holdSeed: false, pendingSeed: null, holdStart: false, pendingStart: null, nextSession: 0,
      ports: [
        { device: device('/dev/ttyACM0', 0x26ac, 0x0039, 'PX4 flight controller'), suggested_role: 'flight_controller', unavailable_reason: 'Flight controller — not a base receiver' },
        { device: device('/dev/ttyACM1', 0x1546, 0x01a9, 'u-blox F9P'), suggested_role: 'rtk_base_station', unavailable_reason: null },
        { device: device('/dev/ttyUSB0', 0x1a86, 0x7523, 'CH340 bridge'), suggested_role: 'other', unavailable_reason: null },
      ],
      base: { session_id: null, revision: 0, phase: 'disconnected', source: null, serial: null, mode: null, receiver: null, survey_in: null, seconds_since_data: null, seconds_since_rtcm: null, error: null },
      link: { link_state: 'disconnected', endpoint: '', fc_alive: false },
      forwarding: { revision: 1, stats: null, foreign: { active: false, packets: 0 } },
      emit(event, payload) {
        for (const listener of listeners.values()) {
          if (listener.event === event) callbacks.get(listener.handler)?.({ event, id: listener.id, payload: structuredClone(payload) })
        }
      },
      updateBase(patch) {
        this.base = { ...this.base, ...patch, revision: this.base.revision + 1 }
        this.emit('rtk_base_status', this.base)
      },
      updateForwarding(patch) {
        this.forwarding = { ...this.forwarding, ...patch, revision: this.forwarding.revision + 1 }
        this.emit('rtk_link_status', this.forwarding)
      },
      setLink(connected) {
        this.link = { link_state: connected ? 'connected' : 'disconnected', endpoint: connected ? 'serial:/dev/ttyACM0:115200' : '', fc_alive: connected }
        this.emit('link', this.link)
      },
    }
    window.__rtk = fixture
    window.__TAURI_INTERNALS__ = {
      transformCallback(callback) { const id = ++next; callbacks.set(id, callback); return id },
      unregisterCallback(id) { callbacks.delete(id) },
      async invoke(command, args = {}) {
        fixture.calls.push({ command, args })
        if (command === 'plugin:event|listen') { const id = ++next; listeners.set(id, { ...args, id }); return id }
        if (command === 'plugin:event|unlisten') { listeners.delete(args.eventId); return }
        if (command === 'link_status') return fixture.link
        if (command === 'enumerate_devices') return fixture.ports.map((p) => p.device)
        if (command === 'rtk_serial_ports') return structuredClone(fixture.ports)
        if (command === 'rtk_base_status') {
          if (fixture.holdSeed) return new Promise((resolve) => { fixture.pendingSeed = resolve })
          return structuredClone(fixture.base)
        }
        if (command === 'rtk_link_status') return structuredClone(fixture.forwarding)
        if (command === 'rtk_start_serial') {
          const id = String(++fixture.nextSession)
          fixture.base = { ...fixture.base, revision: 0, session_id: id }
          fixture.updateBase({ phase: args.mode.kind === 'preconfigured' ? 'ready' : 'identifying',
            source: args.device.port_name, serial: { port_name: args.device.port_name, baud_rate: args.baudRate },
            mode: args.mode, receiver: null, survey_in: null, seconds_since_data: null, seconds_since_rtcm: null, error: null })
          if (fixture.holdStart) return new Promise((resolve) => { fixture.pendingStart = () => resolve(id) })
          return id
        }
        if (command === 'rtk_stop') {
          if (args.sessionId !== fixture.base.session_id) throw 'Base session has changed'
          if (fixture.stopError) throw fixture.stopError
          fixture.updateBase({ phase: 'stopped' })
          fixture.updateForwarding({ stats: null })
          return null
        }
        if (['alert_snapshot', 'tools_status', 'flight_mode_table'].includes(command)) return []
        return null
      },
    }
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} }
    setInterval(() => {
      if (['identifying', 'configuring', 'surveying', 'ready'].includes(fixture.base.phase)) fixture.updateBase({})
    }, 250)
  })

  async function open() {
    await page.getByRole('button', { name: 'Continue without a project', exact: true }).click()
    await page.getByTestId('rtk-pill').click()
  }
  await page.goto(`${url}${url.includes('?') ? '&' : '?'}nomap`, { waitUntil: 'networkidle' })
  await open()
  const drawer = page.getByRole('dialog', { name: 'RTK GPS' })
  const port = drawer.getByLabel('Serial port', { exact: true })
  const accuracy = drawer.getByLabel('Target accuracy (m)', { exact: true })
  const duration = drawer.getByLabel('Minimum observation time (s)', { exact: true })
  const connect = drawer.getByRole('button', { name: 'Connect base', exact: true })
  await until(() => connect.isEnabled(), 'known F9P can connect without a vehicle')
  assert.equal(await port.inputValue(), '/dev/ttyACM1', 'do not select the first (FC) port')
  assert.equal(await port.locator('option[value="/dev/ttyACM0"]').evaluate((option) => option.disabled), true, 'FC option is disabled')
  assert.match(await port.locator('option[value="/dev/ttyUSB0"]').innerText(), /Unidentified/)
  assert.equal(Number(await accuracy.inputValue()), 2)
  assert.equal(Number(await duration.inputValue()), 180)
  assert.equal(await drawer.getByLabel('Baud rate', { exact: true }).inputValue(), '0')
  await shot('survey-in-settings-1920x1080.png')

  for (const invalid of ['0', '361', '10.5', '']) {
    await duration.fill(invalid)
    assert.ok(await connect.isDisabled(), `invalid duration ${invalid}`)
  }
  await duration.fill('180')
  for (const invalid of ['0', '5.01', '']) {
    await accuracy.fill(invalid)
    assert.ok(await connect.isDisabled(), `invalid accuracy ${invalid}`)
  }
  await accuracy.fill('2.00')
  assert.equal(await page.evaluate(() => window.__rtk.calls.filter((c) => c.command === 'rtk_start_serial').length), 0)
  await connect.click()
  await until(() => drawer.getByRole('button', { name: 'Cancel connection' }).isEnabled(), 'configuration can be cancelled')
  assert.ok(await accuracy.isDisabled())
  const start = await page.evaluate(() => window.__rtk.calls.find((c) => c.command === 'rtk_start_serial').args)
  assert.equal(start.device.port_name, '/dev/ttyACM1')
  assert.equal(start.device.serial_number, 'receiver-A')
  assert.equal(start.baudRate, 0)
  assert.deepEqual(start.mode, { kind: 'survey_in', config: { accuracy_limit_m: 2, min_duration_s: 180 } })

  await page.evaluate(() => window.__rtk.updateBase({ phase: 'surveying', receiver: 'ZED-F9P · HPG 1.32 · UBX 27.31',
    survey_in: { duration_s: 60, mean_accuracy_m: 1, observations: 300, valid: false, active: true }, seconds_since_data: 0.1 }))
  await until(() => drawer.getByText('60 / 180 s', { exact: true }).isVisible(), 'accuracy alone does not complete the survey')
  assert.ok(await drawer.getByText('Survey-In in progress', { exact: true }).isVisible())
  await page.evaluate(() => window.__rtk.updateBase({ survey_in: { duration_s: 240, mean_accuracy_m: 3, observations: 1200, valid: false, active: true } }))
  await until(() => drawer.getByText('240 / 180 s', { exact: true }).isVisible(), 'elapsed duration is not a forced end')
  assert.ok(await drawer.getByText('Survey-In in progress', { exact: true }).isVisible())
  assert.ok(await drawer.getByText('Waiting for a vehicle link. Base observations continue.', { exact: true }).isVisible())
  assert.equal(await drawer.getByText('No RTCM data for over 5 s', { exact: false }).count(), 0, 'no correction-stall alarm during Survey-In')
  await page.setViewportSize({ width: 1024, height: 640 })
  await drawer.getByText('Survey-In in progress', { exact: true }).scrollIntoViewIfNeeded()
  const stopBox = await drawer.getByRole('button', { name: 'Disconnect base' }).boundingBox()
  assert.ok(stopBox.y >= 0 && stopBox.y + stopBox.height <= 640, 'Stop is pinned in the viewport')
  await shot('survey-in-progress-1024x640.png')

  await page.setViewportSize({ width: 1920, height: 1080 })
  await page.evaluate(() => {
    const f = window.__rtk
    f.updateBase({ phase: 'ready', survey_in: { duration_s: 246, mean_accuracy_m: 1.98, observations: 1230, valid: true, active: false }, seconds_since_rtcm: 0.2 })
    f.setLink(true)
    f.updateForwarding({ stats: { source: '/dev/ttyACM1', base_session_id: f.base.session_id, connection_generation: 1,
      bytes_per_second: 322, frames_per_type: { 1005: 1, 1077: 3 }, dropped_chunks: 0, total_frames: 4, stalled: false, seconds_since_data: 0.1, error: null } })
    f.emit('telemetry', { gps: { fix_type: 'RTK_FLOAT', satellites_visible: 18 }, field_ages: { gps_at_ms: Date.now() } })
  })
  await until(() => drawer.getByText('322', { exact: true }).isVisible(), 'matching-session link stats displayed')
  assert.ok(await drawer.getByText('1005×1 1077×3', { exact: true }).isVisible())
  assert.ok(await drawer.getByText('Base ready', { exact: true }).isVisible())
  assert.ok(await drawer.getByText('RTK FLOAT', { exact: true }).isVisible(), 'base readiness does not manufacture rover FIXED')
  await shot('survey-in-ready-1920x1080.png')

  await page.evaluate(async () => {
    const { applyFieldMode, applyThemePref } = await import('/src/design-system/theme.ts')
    applyFieldMode(true)
    applyThemePref('light')
    window.__rtk.updateBase({ seconds_since_data: 0.1, seconds_since_rtcm: 6 })
    window.__rtk.updateForwarding({
      stats: { ...window.__rtk.forwarding.stats, bytes_per_second: 0, frames_per_type: {} },
      foreign: { active: true, packets: 5 },
    })
  })
  await until(() => drawer.getByText('No RTCM data for over 5 s', { exact: false }).isVisible(), 'RTCM can stall while raw source remains active')
  assert.equal(await drawer.getByText('No source bytes for over 5 s', { exact: false }).count(), 0)
  await drawer.getByText('Last valid RTCM', { exact: true }).scrollIntoViewIfNeeded()
  await shot('survey-in-field-light.png')

  await page.evaluate(() => {
    const f = window.__rtk
    f.updateForwarding({ stats: null, foreign: { active: false, packets: 0 } })
    f.setLink(false)
    f.stopError = 'Receiver failed to stop; retry disconnect'
    f.emit('rtk_link_status', { revision: 1, stats: { source: 'obsolete', bytes_per_second: 999 }, foreign: { active: true, packets: 99 } })
  })
  assert.ok(await drawer.getByRole('button', { name: 'Disconnect base' }).isEnabled())
  await until(() => drawer.getByText('Waiting for a vehicle link. Base observations continue.', { exact: true }).isVisible(), 'vehicle disconnect preserves base')
  assert.ok(await drawer.getByText('Base ready', { exact: true }).isVisible())
  assert.equal(await drawer.getByText('Another injector is sending RTCM', { exact: false }).count(), 0)
  await drawer.getByRole('button', { name: 'Disconnect base' }).click()
  await until(() => drawer.getByRole('alert').isVisible(), 'failed stop is visible')
  assert.match(await drawer.getByRole('alert').innerText(), /failed to stop/)
  assert.ok(await drawer.getByRole('button', { name: 'Disconnect base' }).isEnabled(), 'stop is retryable without FC')
  await page.evaluate(() => { window.__rtk.stopError = null })
  await drawer.getByRole('button', { name: 'Disconnect base' }).click()
  await until(() => connect.isEnabled(), 'successful Stop releases settings')
  const stoppedId = await page.evaluate(() => window.__rtk.calls.find((c) => c.command === 'rtk_stop').args.sessionId)
  assert.equal(stoppedId, '1')

  // Same OS port, different USB identity: require a new operator selection.
  await page.evaluate(() => {
    window.__rtk.ports[1].device.serial_number = 'receiver-B'
    window.__rtk.emit('devices_changed', window.__rtk.ports.map((p) => p.device))
  })
  await until(async () => await port.inputValue() === '', 'replug does not silently select a replacement')
  assert.ok(await connect.isDisabled())
  await port.selectOption('/dev/ttyACM1')
  await drawer.getByLabel('Receiver mode', { exact: true }).selectOption('preconfigured')
  assert.equal(await accuracy.count(), 0)
  assert.equal(await drawer.getByLabel('Baud rate', { exact: true }).inputValue(), '115200')
  await connect.click()
  await until(() => drawer.getByText('Reading receiver output', { exact: true }).isVisible(), 'passive receiver remains a distinct mode')
  const lastStart = await page.evaluate(() => window.__rtk.calls.filter((c) => c.command === 'rtk_start_serial').at(-1).args)
  assert.deepEqual(lastStart.mode, { kind: 'preconfigured' })
  assert.equal(lastStart.device.serial_number, 'receiver-B')
  await drawer.getByRole('button', { name: 'Disconnect base' }).click()

  // An older session must not revive through a late cached command response.
  await page.evaluate(() => localStorage.removeItem('maggcs.rtk.survey-in'))
  await page.reload({ waitUntil: 'networkidle' })
  await page.getByRole('button', { name: 'Continue without a project', exact: true }).click()
  await page.evaluate(() => { window.__rtk.holdSeed = true })
  await page.getByTestId('rtk-pill').click()
  await until(() => page.evaluate(() => !!window.__rtk.pendingSeed), 'seed is held')
  await page.evaluate(() => {
    const f = window.__rtk
    f.updateBase({ session_id: '10', phase: 'surveying', source: 'new-base', mode: { kind: 'survey_in', config: { accuracy_limit_m: 2, min_duration_s: 180 } },
      survey_in: { duration_s: 42, mean_accuracy_m: 3, observations: 210, valid: false, active: true } })
    f.holdSeed = false
    f.pendingSeed({ ...f.base, session_id: '1', revision: 99999, phase: 'ready', receiver: 'OBSOLETE RECEIVER' })
  })
  await until(() => drawer.getByText('42 / 180 s', { exact: true }).isVisible(), 'live session wins over cached older session')
  assert.equal(await drawer.getByText('OBSOLETE RECEIVER', { exact: true }).count(), 0)

  // Cancel after the starting event but before the Start IPC reply arrives.
  await page.reload({ waitUntil: 'networkidle' })
  await open()
  await page.evaluate(() => { window.__rtk.holdStart = true })
  await until(() => connect.isEnabled(), 'ready to start')
  await connect.click()
  await until(() => drawer.getByRole('button', { name: 'Cancel connection' }).isEnabled(), 'pending reply still allows cancellation')
  await drawer.getByRole('button', { name: 'Cancel connection' }).click()
  await until(() => connect.isEnabled(), 'cancel completed')
  await page.evaluate(() => window.__rtk.pendingStart())
  await page.waitForTimeout(100)
  assert.ok(await connect.isEnabled(), 'late start reply cannot resurrect a cancelled session')
  assert.equal(await drawer.getByText('Identifying receiver', { exact: true }).count(), 0)
  await page.keyboard.press('Escape')
  assert.equal(await drawer.count(), 0)
  assert.deepEqual(errors, [])
  console.log('PASS RTK: QGC defaults, invalid inputs, FC exclusion, identity change, offline survey, dual targets, receiver/rover separation, RTCM freshness, forwarding, stop/retry, stale sessions, cancelled late Start, compact/desktop/Field screenshots')
} catch (error) {
  console.error('RTK page errors:', errors)
  console.error(await page.locator('body').innerText())
  throw error
} finally {
  await browser.close()
}
