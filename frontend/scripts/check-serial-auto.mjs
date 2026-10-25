// Real React controls with a revisioned native IPC fixture. No device is opened.
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
const until = async (test, label) => {
  const end = Date.now() + 7000
  while (!await test()) { assert.ok(Date.now() < end, label); await page.waitForTimeout(30) }
}
const shot = async (name) => { if (shots) await page.screenshot({ path: join(shots, name) }) }

try {
  if (shots) await mkdir(shots, { recursive: true })
  await page.addInitScript(() => {
    localStorage.setItem('maggcs.autoconnect', 'true')
    if (!localStorage.getItem('__nativeSerialProfile')) localStorage.setItem('maggcs.rtk.survey-in', JSON.stringify({ kind: 'survey_in', accuracy: '1.50', duration: '240', baud: 0 }))
    const callbacks = new Map(), listeners = new Map()
    let callback = 0, revision = 10
    const next = () => String(++revision)
    const makePort = (instance, name, serial, role, vendor, product) => {
      const device = { port_name: name, transport: 'usb', device_id: { vendor_id: vendor, product_id: product }, serial_number: serial, manufacturer: null, product: role === 'flight_controller' ? 'PX4 flight controller' : 'u-blox F9P' }
      return { port: { instance_id: String(instance), device }, stable_id: { device_id: device.device_id, serial_number: serial }, suggested_role: role, lease: null }
    }
    const policy = () => ({ enabled: true, paused_by_user: false, saved: null })
    const settings = JSON.parse(localStorage.getItem('__nativeSerialProfile') ?? 'null') ?? {
      version: 1, fc: { policy: policy(), baud: { kind: 'auto' }, expected_target: null },
      rtk: { policy: policy(), receiver: { baud: { kind: 'auto' }, mode: { kind: 'survey_in', config: { accuracy_limit_m: 2, min_duration_s: 180 } } } },
    }
    const state = (role, policy) => ({ intent: { role, id: next() }, revision: next(), phase: policy.paused_by_user ? 'paused' : 'waiting_for_device', attempt: null, port: null, session: null, retry_in_ms: null, error: null })
    const fixture = {
      calls: [], bootstrap: false,
      inventory: { revision: next(), health: { kind: 'ready' }, ports: [makePort(1, '/dev/ttyACM0', 'FC-A', 'flight_controller', 0x26ac, 0x0039), makePort(2, '/dev/ttyACM1', 'BASE-A', 'rtk_base_station', 0x1546, 0x01a9)] },
      serial: { revision: next(), settings_revision: next(), settings, primary_startup_owner: 'serial_discovery', fc: state('primary_fc', settings.fc.policy), rtk: state('rtk_base', settings.rtk.policy), settings_error: null },
      base: { session_id: null, revision: 0, phase: 'disconnected', source: null, serial: null, mode: null, receiver: null, survey_in: null, seconds_since_data: null, seconds_since_rtcm: null, error: null, failure_kind: null },
      link: { link_state: 'disconnected', endpoint: '', fc_alive: false },
      forwarding: { revision: 1, stats: null, foreign: { active: false, packets: 0 } },
      emit(event, payload) {
        for (const listener of listeners.values()) if (listener.event === event) callbacks.get(listener.handler)?.({ event, id: listener.id, payload: structuredClone(payload) })
      },
      publish() { this.serial.revision = next(); this.emit('serial_auto_connect_status', this.serial) },
      persist() { localStorage.setItem('__nativeSerialProfile', JSON.stringify(this.serial.settings)) },
      ports() {
        return this.inventory.ports.map((p) => ({ device: p.port.device, port_ref: p.port, suggested_role: p.suggested_role,
          unavailable_reason: p.lease ? 'In use by a connection' : p.suggested_role === 'flight_controller' ? 'Flight controller — not a base receiver' : null }))
      },
      changedInventory() { this.inventory.revision = next(); this.emit('serial_inventory', this.inventory) },
      setBase(patch) { this.base = { ...this.base, ...patch, revision: this.base.revision + 1 }; this.emit('rtk_base_status', this.base) },
      startBase(mode, port = this.inventory.ports[1].port) {
        const id = next()
        this.setBase({ session_id: id, phase: 'surveying', source: port.device.port_name, serial: { port_name: port.device.port_name, baud_rate: 0 }, mode,
          receiver: 'ZED-F9P · HPG 1.32 · UBX 27.31', survey_in: { duration_s: 42, mean_accuracy_m: 2.5, observations: 210, active: true, valid: false }, seconds_since_data: 0.1 })
        this.serial.rtk = { ...this.serial.rtk, phase: 'running', port, session: { kind: 'base', id }, error: null }
        this.inventory.ports[1].lease = { id: next(), owner: 'rtk_base', phase: 'active' }
        this.changedInventory(); this.publish()
        return id
      },
      pause(role) {
        const key = role === 'primary_fc' ? 'fc' : 'rtk'
        this.serial.settings[key].policy.paused_by_user = true
        this.serial[key] = { ...this.serial[key], intent: { role, id: next() }, phase: 'paused', session: null }
        this.serial.settings_revision = next()
        if (key === 'rtk') { this.setBase({ phase: 'stopped' }); this.inventory.ports[1].lease = null; this.changedInventory() }
        else { this.link = { link_state: 'disconnected', endpoint: '', fc_alive: false }; this.emit('link', this.link) }
        this.persist(); this.publish()
      },
      replace(role, sameName = false) {
        const index = role === 'primary_fc' ? 0 : 1
        const old = this.inventory.ports[index]
        this.inventory.ports[index] = makePort(next(), sameName ? old.port.device.port_name : '/dev/ttyACM2', `${old.port.device.serial_number}-NEW`, old.suggested_role, old.port.device.device_id.vendor_id, old.port.device.device_id.product_id)
        this.changedInventory()
      },
    }
    window.__serial = fixture
    window.__TAURI_INTERNALS__ = {
      transformCallback(fn) { const id = ++callback; callbacks.set(id, fn); return id },
      unregisterCallback(id) { callbacks.delete(id) },
      async invoke(command, args = {}) {
        fixture.calls.push({ command, args: structuredClone(args) })
        if (command === 'plugin:event|listen') { const id = ++callback; listeners.set(id, { ...args, id }); return id }
        if (command === 'plugin:event|unlisten') { listeners.delete(args.eventId); return }
        if (command === 'serial_inventory') return structuredClone(fixture.inventory)
        if (command === 'serial_auto_connect_status') {
          if (!fixture.bootstrap) {
            fixture.bootstrap = true
            if (!localStorage.getItem('__nativeSerialProfile') && args.legacyRtk) fixture.serial.settings.rtk.receiver = args.legacyRtk
            fixture.persist(); fixture.publish()
          }
          return structuredClone(fixture.serial)
        }
        if (command === 'serial_auto_connect_save') {
          if (args.expected !== fixture.serial.settings_revision) throw 'settings revision changed'
          fixture.serial.settings = args.settings; fixture.serial.settings_revision = next(); fixture.persist(); fixture.publish()
          return structuredClone(fixture.serial)
        }
        if (command === 'serial_auto_connect_pause' || command === 'serial_auto_connect_resume') {
          const key = args.expected.role === 'primary_fc' ? 'fc' : 'rtk'
          if (args.expected.id !== fixture.serial[key].intent.id) throw 'intent changed'
          if (command.endsWith('pause')) fixture.pause(args.expected.role)
          else {
            fixture.serial.settings[key].policy.paused_by_user = false
            fixture.serial[key] = { ...fixture.serial[key], intent: { role: args.expected.role, id: next() }, phase: 'probing' }
            fixture.serial.settings_revision = next(); fixture.persist(); fixture.publish()
          }
          return structuredClone(fixture.serial[key])
        }
        if (command === 'rtk_serial_ports') return structuredClone(fixture.ports())
        if (command === 'rtk_base_status') return structuredClone(fixture.base)
        if (command === 'rtk_link_status') return fixture.forwarding
        if (command === 'rtk_stop') {
          if (args.sessionId !== fixture.base.session_id || args.expectedIntent?.id !== fixture.serial.rtk.intent.id) throw 'stale base stop'
          fixture.pause('rtk_base'); return null
        }
        if (command === 'rtk_start_serial') {
          const port = fixture.inventory.ports.find((p) => p.port.instance_id === args.port?.instance_id)
          if (!port || port.port.device.serial_number !== args.device.serial_number) throw 'stale manual selection'
          fixture.serial.rtk.intent = { role: 'rtk_base', id: next() }
          fixture.serial.settings.rtk.policy.paused_by_user = false
          fixture.serial.settings.rtk.receiver = { baud: args.baudRate === 0 ? { kind: 'auto' } : { kind: 'fixed', rate: args.baudRate }, mode: args.mode }
          fixture.persist()
          return fixture.startBase(args.mode, args.port)
        }
        if (command === 'connect') {
          if (args.endpoint.startsWith('serial:') && !fixture.inventory.ports.some((p) => p.port.instance_id === args.expectedSerial?.instance_id)) throw 'stale manual FC'
          fixture.link = { link_state: 'connected', endpoint: args.endpoint, fc_alive: true }
          fixture.serial.fc = { ...fixture.serial.fc, intent: { role: 'primary_fc', id: next() }, phase: 'running', port: args.expectedSerial, session: { kind: 'fc', connection_id: '10', generation: '1' } }
          fixture.serial.settings.fc.policy.paused_by_user = false
          fixture.emit('link', fixture.link); fixture.persist(); fixture.publish()
          return { ok: true, endpoint: args.endpoint, target_system_id: 1, target_component_id: 1, target_discovered: true }
        }
        if (command === 'disconnect') { fixture.pause('primary_fc'); return null }
        if (command === 'teardown_guard') return { reason: args.reason, refusal: null, keyword: null, forced: false, armed: false, on_ground: true }
        if (command === 'link_status') return fixture.link
        if (command === 'enumerate_devices') return fixture.inventory.ports.map((p) => p.port.device)
        if (['alert_snapshot', 'tools_status', 'flight_mode_table'].includes(command)) return []
        return null
      },
    }
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} }
  })

  await page.goto(`${url}${url.includes('?') ? '&' : '?'}nomap`, { waitUntil: 'networkidle' })
  await page.getByRole('button', { name: 'Continue without a project', exact: true }).click()
  await page.getByTestId('rtk-pill').click()
  const drawer = page.getByRole('dialog', { name: 'RTK GPS' })
  const rtkAuto = page.getByTestId('serial-auto-rtk_base')
  await until(() => rtkAuto.isVisible(), 'native auto status is hydrated')
  const hud = page.getByTestId('rtk-gps-dashboard')
  assert.equal(await hud.getByTestId('rtk-hud-fix').getAttribute('data-state'), 'unavailable')
  assert.ok(await rtkAuto.getByRole('checkbox').isChecked(), 'auto enabled by default')
  await page.waitForTimeout(3200)
  assert.equal(await page.evaluate(() => window.__serial.calls.filter((c) => c.command === 'connect' || c.command === 'rtk_start_serial').length), 0, 'native owner suppresses legacy timer and frontend auto starts')
  assert.equal(await page.evaluate(() => window.__serial.serial.settings.rtk.receiver.mode.config.accuracy_limit_m), 1.5, 'legacy settings imported before auto start')
  await page.evaluate(() => window.__serial.startBase(window.__serial.serial.settings.rtk.receiver.mode))
  await until(() => drawer.getByRole('button', { name: 'Disconnect base', exact: true }).isEnabled(), 'automatic base is manually stoppable')
  assert.equal(await hud.getByRole('progressbar', { name: 'Observation time', exact: true }).getAttribute('aria-valuenow'), '18')
  await page.evaluate(() => window.__serial.setBase({ survey_in: { duration_s: 240, mean_accuracy_m: 1, observations: 1200, active: true, valid: false } }))
  await until(async () => await hud.getByRole('progressbar', { name: 'Observation time', exact: true }).getAttribute('aria-valuenow') === '100', 'time target met')
  assert.ok(await hud.getByText('Awaiting receiver validity', { exact: true }).isVisible(), 'two full bars are not receiver completion')
  await page.evaluate(() => {
    const f = window.__serial
    f.setBase({ phase: 'ready', seconds_since_rtcm: 0.1, survey_in: { duration_s: 241, mean_accuracy_m: 1, observations: 1205, active: false, valid: true } })
    f.link = { link_state: 'connected', endpoint: 'serial:/dev/ttyACM0:115200', fc_alive: true }
    f.emit('link', f.link)
    f.emit('telemetry', { gps: { fix_type: 'RTK_FIXED', satellites_visible: 18, hdop: 0.65, vdop: 1.05 }, field_ages: { gps_at_ms: Date.now() } })
    f.forwarding = { revision: 2, foreign: { active: false, packets: 0 }, stats: { source: f.base.source, base_session_id: f.base.session_id, connection_generation: 1,
      bytes_per_second: 100, frames_per_type: { 1005: 1 }, total_frames: 1, dropped_chunks: 0, seconds_since_data: 0.1, stalled: false, error: null } }
    f.emit('rtk_link_status', f.forwarding)
  })
  await until(async () => await hud.getByTestId('rtk-hud-fix').getAttribute('data-state') === 'live', 'fresh rover solution')
  assert.ok(await hud.getByText('FIXED', { exact: true }).isVisible())
  assert.ok(await hud.getByText('Survey valid', { exact: true }).isVisible())
  await page.waitForTimeout(1100)
  await page.evaluate(() => { const f = window.__serial; f.forwarding.revision += 1; f.forwarding.stats.bytes_per_second = 220; f.forwarding.stats.total_frames = 2; f.emit('rtk_link_status', f.forwarding) })
  await until(async () => Number(await hud.getByTestId('rtk-hud-trend').getAttribute('data-samples')) >= 2, 'measured TX history')
  await hud.scrollIntoViewIfNeeded()
  await shot('rtk-gps-hud-1920x1080.png')
  await page.setViewportSize({ width: 1024, height: 640 })
  await hud.scrollIntoViewIfNeeded()
  await shot('rtk-gps-hud-1024x640.png')
  await page.setViewportSize({ width: 1366, height: 768 })
  await page.evaluate(async () => { const { applyFieldMode, applyThemePref } = await import('/src/design-system/theme.ts'); applyFieldMode(true); applyThemePref('light') })
  await hud.scrollIntoViewIfNeeded()
  await shot('rtk-gps-hud-field-light-1366x768.png')
  await page.evaluate(async () => { const { applyFieldMode, applyThemePref } = await import('/src/design-system/theme.ts'); applyFieldMode(false); applyThemePref('dark') })
  await page.setViewportSize({ width: 1920, height: 1080 })
  await page.evaluate(() => {
    const f = window.__serial
    f.emit('telemetry', { gps: { fix_type: 'RTK_FIXED', satellites_visible: 18, hdop: 0.65, vdop: 1.05 }, field_ages: { gps_at_ms: Date.now() - 10_000 } })
    f.setBase({ seconds_since_rtcm: 8 })
  })
  await until(async () => await hud.getByTestId('rtk-hud-fix').getAttribute('data-state') === 'stale', 'old FIXED is visibly stale')
  assert.equal(await hud.getByTestId('rtk-hud-age').getAttribute('data-state'), 'stale')
  await drawer.getByRole('button', { name: 'Disconnect base', exact: true }).click()
  await until(() => rtkAuto.getByText('Paused by operator', { exact: true }).isVisible(), 'manual Stop pauses native policy')
  await until(async () => await hud.getByTestId('rtk-hud-trend').getAttribute('data-samples') === '0', 'retired session clears TX history')
  await page.setViewportSize({ width: 1024, height: 640 })
  await rtkAuto.scrollIntoViewIfNeeded()
  await shot('rtk-paused-1024x640.png')
  await page.evaluate(() => window.__serial.replace('rtk_base'))
  await until(async () => await drawer.getByLabel('Serial port', { exact: true }).inputValue() === '', 'replacement invalidates manual selection')
  assert.equal(await page.evaluate(() => window.__serial.calls.filter((c) => c.command === 'rtk_start_serial').length), 0)
  const old = await page.evaluate(() => structuredClone(window.__serial.serial))
  await rtkAuto.getByRole('button', { name: 'Resume auto', exact: true }).click()
  await until(() => rtkAuto.getByText('Identifying device', { exact: true }).isVisible(), 'explicit native Resume')
  await page.evaluate((old) => window.__serial.emit('serial_auto_connect_status', old), old)
  assert.ok(await rtkAuto.getByText('Identifying device', { exact: true }).isVisible(), 'old snapshot cannot restore pause')
  await drawer.getByLabel('Serial port', { exact: true }).selectOption('/dev/ttyACM2')
  await drawer.getByLabel('Target accuracy (m)', { exact: true }).fill('2.25')
  await drawer.getByLabel('Minimum observation time (s)', { exact: true }).fill('300')
  await drawer.getByRole('button', { name: 'Connect base', exact: true }).click()
  await until(() => drawer.getByRole('button', { name: 'Disconnect base', exact: true }).isEnabled(), 'manual Connect remains available')
  const baseStart = await page.evaluate(() => window.__serial.calls.find((c) => c.command === 'rtk_start_serial').args)
  assert.ok(baseStart.port.instance_id && baseStart.port.device.serial_number.endsWith('-NEW'))
  assert.equal(baseStart.mode.config.accuracy_limit_m, 2.25)
  await page.setViewportSize({ width: 1920, height: 1080 })
  await shot('rtk-manual-1920x1080.png')
  await drawer.getByRole('button', { name: 'Disconnect base', exact: true }).click()
  await page.reload({ waitUntil: 'networkidle' })
  await page.getByRole('button', { name: 'Continue without a project', exact: true }).click()
  await page.getByTestId('rtk-pill').click()
  await until(() => page.getByTestId('serial-auto-rtk_base').getByText('Paused by operator', { exact: true }).isVisible(), 'pause survives rehydration')
  await page.keyboard.press('Escape')

  await page.getByTestId('settings-open').click()
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true })
  await until(() => settings.isVisible(), 'manual settings')
  await settings.getByRole('tab', { name: 'Connection', exact: true }).click()
  await settings.getByTestId('manual-link-kind').selectOption('serial')
  await settings.getByTestId('manual-link-port').selectOption('/dev/ttyACM0')
  const connect = settings.getByTestId('manual-link-connect')
  await until(() => connect.isEnabled(), 'manual FC ready')
  await page.evaluate(() => window.__serial.replace('primary_fc', true))
  await until(() => connect.isDisabled(), 'same-path replacement invalidates manual FC selection')
  await settings.getByRole('button', { name: 'Use current device', exact: true }).click()
  await until(() => connect.isEnabled(), 'operator reselects current attachment')
  await connect.click()
  await until(() => page.evaluate(() => window.__serial.link.link_state === 'connected'), 'manual FC connected')
  const fcStart = await page.evaluate(() => window.__serial.calls.find((c) => c.command === 'connect').args)
  assert.equal(fcStart.expectedSerial.device.serial_number, 'FC-A-NEW')
  const fcAuto = settings.getByTestId('serial-auto-primary_fc')
  assert.ok(await fcAuto.getByRole('checkbox').isChecked())
  assert.ok(await settings.getByTestId('settings-disconnect').isVisible(), 'manual Disconnect remains visible')
  await settings.getByTestId('manual-link-connect').scrollIntoViewIfNeeded()
  await shot('fc-manual-controls-1920x1080.png')
  await fcAuto.getByRole('button', { name: 'Disconnect / pause auto', exact: true }).click()
  await until(() => fcAuto.getByText('Paused by operator', { exact: true }).isVisible(), 'FC disconnect pauses auto')
  await page.setViewportSize({ width: 1366, height: 768 })
  await page.evaluate(async () => { const { applyFieldMode, applyThemePref } = await import('/src/design-system/theme.ts'); applyFieldMode(true); applyThemePref('light') })
  await fcAuto.scrollIntoViewIfNeeded()
  await page.waitForTimeout(3500)
  await shot('fc-manual-field-light-1366x768.png')
  assert.deepEqual(errors, [])
  console.log('PASS serial auto: native startup owner, default-on policy, legacy import, manual override, attachment identity, pause/reload/resume, stale snapshots and screenshots')
} catch (error) {
  console.error('Page errors:', errors)
  console.error(await page.locator('body').innerText())
  throw error
} finally { await browser.close() }
