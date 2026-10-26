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
const fcRangeOnly = process.env.MOTOR_FC_RANGE_ONLY === '1'
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
  const streamCatalog = [
    { id: 1, name: 'SYS_STATUS', fields: [{ name: 'current_battery', wire_type: 'int16_t', units: 'cA' }] },
    { id: 24, name: 'GPS_RAW_INT', fields: [{ name: 'fix_type', wire_type: 'uint8_t', units: null }] },
    { id: 26, name: 'SCALED_IMU', fields: [{ name: 'xacc', wire_type: 'int16_t', units: 'mG' }] },
    { id: 36, name: 'SERVO_OUTPUT_RAW', fields: [{ name: 'servo1_raw', wire_type: 'uint16_t', units: 'us' }] },
    { id: 43, name: 'MISSION_REQUEST_LIST', fields: [{ name: 'target_system', wire_type: 'uint8_t', units: null }], px4_support: 'unknown' },
    { id: 105, name: 'HIGHRES_IMU', fields: [{ name: 'xmag', wire_type: 'float', units: 'gauss' }] },
    { id: 147, name: 'BATTERY_STATUS', fields: [{ name: 'current_battery', wire_type: 'int16_t', units: 'cA' }, { name: 'voltages', wire_type: 'uint16_t[10]', units: 'mV' }] },
    { id: 291, name: 'ESC_STATUS', fields: [{ name: 'rpm', wire_type: 'int32_t[4]', units: 'rpm' }] },
  ].map((message) => {
    const sources = {
      24: ['sensor_gps'], 26: ['vehicle_imu', 'vehicle_imu_status', 'sensor_mag'],
      36: ['actuator_outputs'], 105: ['vehicle_imu', 'sensor_selection', 'vehicle_magnetometer', 'vehicle_air_data', 'differential_pressure', 'estimator_sensor_bias', 'estimator_selector_status'],
      147: ['battery_status'], 291: ['esc_status'],
    }[message.id]
    return { px4_support: 'documented_v1_17', ...message, px4_mapping: sources ? {
      uorb_sources: sources,
      source_url: `https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/modules/mavlink/streams/${message.name}.hpp`,
      note: message.id === 26 ? 'Instance 0; integrated accel/gyro and sensor-mag values, with wire scaling.' : 'Selected fields from the verified PX4 source; availability and rate depend on source updates.',
    } : null }
  })
  // Local storage here represents the isolated test backend's persisted file,
  // not application-side restoration of interval commands.
  const savedStreams = JSON.parse(localStorage.getItem('test.backend.streams') ?? '[]')
  const initialStreams = new Map([[36, 20], [105, 50], [147, 5], [291, 10]].map(([id, hz]) => [id, { message_id: id, rate: { kind: 'hertz', hz }, allow_unknown_support: false }]))
  for (const request of savedStreams) initialStreams.set(request.message_id, request)
  const streamRow = (request) => ({ message_id: request.message_id, requested_rate: request.rate, observed_hz: null, last_seen_age_ms: null, receive_state: 'waiting', error: null })
  const streams = { target: null, endpoint: 'isolated IPC test', transport_generation: 1, ready: true, statuses: [...initialStreams.values()].map(streamRow), error: null }
  const status = {
    motor_control: { enabled: false },
    tones: {
      settings: { start: 'rising', stop: 'falling' }, starting: false, busy: false,
      phase: 'idle', trigger: null, error: null,
    },
    connected: true, fc_alive: true, target_fc_alive: true, armed: false,
    custom_mode: 0, autopilot_px4: true, mode_label: 'Manual', endpoint: 'isolated IPC test',
    tick_hz: 10, session: 'idle', sitl_connected: false, sitl_endpoint: null,
    sitl_source: null, sitl_feed_age_s: null, sitl_origin: null, main_sitl_source: null,
    target_connected: true,
    target_endpoint: 'isolated IPC test', target_system_id: 250,
    target_origin: 'dedicated', main_serial_target: null,
    com_mot_test_en: 1, sdlog_mode: 0, sdlog_backend: 1,
    parameter_restart_pending: false, input_values: [], input_pwm: [],
    output_values: [],
    output_pwm: [],
    fc_output_pwm: [1500, 1503, 1740, null],
    input_values: [0.4, 0.5, 0.6, 0.7],
    input_pwm: [1400, 1500, 1600, 1700],
    pwm_models: [
      { min_us: 1100, max_us: 1900, reversed: false, thrust_factor: 0, disabled: false, range_source: 'fc', protocol: { kind: 'pwm', rate_hz: 400 }, dshot_min: 0 },
      { min_us: 1000, max_us: 2000, reversed: false, thrust_factor: 0, disabled: false, range_source: 'fc', protocol: { kind: 'pwm', rate_hz: 400 }, dshot_min: 0 },
      { min_us: 1000, max_us: 2000, reversed: true, thrust_factor: 0, disabled: false, range_source: 'fc', protocol: { kind: 'pwm', rate_hz: 400 }, dshot_min: 0 },
      { min_us: 1000, max_us: 2000, reversed: false, thrust_factor: 0, disabled: false, range_source: 'fc', protocol: { kind: 'pwm', rate_hz: 400 }, dshot_min: 0 },
    ],
    scope_outputs: [],
    output_protocols: {
      main: { kind: 'pwm', rate_hz: 400 },
      aux: { kind: 'pwm', rate_hz: 400 },
    },
    dshot_3d_enable: 0,
    dshot_bidir_en: 0,
    esc_rpm: [],
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
  let scopeRevision = 0
  let scopeSignature = ''
  let scopeConnection = 1
  const scopeOutputs = () => {
    const connection = window.__bench.connectionId
    if (connection !== scopeConnection) {
      scopeConnection = connection
      scopeRevision = 0
      scopeSignature = ''
    }
    const signature = JSON.stringify([
      status.target_connected, status.target_system_id, status.output_map,
      status.pwm_models, status.output_protocols, status.dshot_3d_enable, status.dshot_bidir_en,
    ])
    if (signature !== scopeSignature) {
      scopeSignature = signature
      scopeRevision += 1
    }
    if (!status.target_connected || status.output_map.state !== 'complete') return []
    const outputs = []
    for (const assignment of status.output_map.motors) {
      if (assignment.motor !== outputs.length + 1 || !assignment.channel) break
      const model = status.pwm_models[outputs.length]
      if (!model || model.protocol.kind === 'unknown') return []
      outputs.push({
        connection_id: connection, revision: scopeRevision, motor: assignment.motor,
        channel: structuredClone(assignment.channel),
        unit: ['dshot', 'bdshot'].includes(model.protocol.kind) ? 'd_shot_throttle' : 'microseconds',
      })
    }
    return outputs
  }
  window.__bench = {
    calls: [], dialogs: [], status, mainConnected: false, connectionId: 1, scopeOutputs, downloadResult: false,
    startError: null, mappingError: null,
    holdCapture: null, pendingCapture: null,
    holdMotorStart: false, pendingMotorStart: null,
    streams, holdStreamWrite: false, pendingStreamWrite: null,
    completeStreamWrite() {
      const pending = this.pendingStreamWrite
      if (!pending) throw new Error('No pending stream request')
      this.pendingStreamWrite = null
      this.holdStreamWrite = false
      pending()
    },
    completeMotorStart() {
      const pending = this.pendingMotorStart
      if (!pending) throw new Error('No pending motor start')
      this.pendingMotorStart = null
      this.holdMotorStart = false
      status.tones.busy = false
      status.tones.starting = false
      if (!pending.cancelled) pending.complete()
    },
    cancelMotorStart() {
      const pending = this.pendingMotorStart
      if (pending && !pending.cancelled) {
        pending.cancelled = true
        status.tones.starting = false
        status.tones.phase = 'cancelled'
        pending.reject('motor start/tone cancelled')
      }
    },
    entries: [{ id: 0, size: 1024, time_utc: 1700000000 }],
    completeCapture() {
      const pending = this.pendingCapture
      if (!pending) throw new Error('No pending capture request')
      this.pendingCapture = null
      this.holdCapture = null
      pending.complete()
    },
    emit(event, payload) {
      if (event === 'actuator_samples') {
        const outputs = scopeOutputs()
        payload = payload.map((sample) => {
          const motor = /^(?:cmd\.pwm|fc\.motor)([1-4])$/.exec(sample.id)
          if (!motor || Object.hasOwn(sample, 'output')) return sample
          return { ...sample, output: outputs.find((output) => output.motor === Number(motor[1])) }
        })
      }
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
      if (command === 'actuator_status') {
        status.scope_outputs = scopeOutputs()
        return structuredClone(status)
      }
      if (command === 'actuator_use_main_target') {
        if (test.mainLinkError) throw new Error(test.mainLinkError)
        if (JSON.stringify(args.expected) !== JSON.stringify(status.main_serial_target)) throw new Error('main serial target changed')
        status.target_connected = true
        status.target_fc_alive = true
        status.connected = true
        status.fc_alive = true
        status.endpoint = args.expected.endpoint
        status.target_origin = 'main_serial'
        status.target_endpoint = args.expected.endpoint
        status.target_system_id = args.expected.target.target_system_id
        status.motor_control.enabled = false
        return
      }
      if (command === 'actuator_disconnect_target') {
        status.target_connected = false
        status.target_fc_alive = false
        status.connected = false
        status.fc_alive = false
        status.endpoint = null
        status.target_origin = null
        status.motor_control.enabled = false
        return
      }
      if (command === 'actuator_use_main_sitl') {
        if (test.mainSitlError) throw new Error(test.mainSitlError)
        if (JSON.stringify(args.expected) !== JSON.stringify(status.main_sitl_source)) throw new Error('main SITL source changed')
        status.sitl_connected = true
        status.sitl_origin = 'main_link'
        status.sitl_endpoint = args.expected.endpoint
        status.sitl_source = {
          endpoint: args.expected.endpoint,
          system_id: args.expected.system_id,
          component_id: args.expected.component_id,
          port: args.port,
        }
        status.sitl_feed_age_s = 0
        return
      }
      if (command === 'actuator_disconnect_sitl') {
        status.sitl_connected = false
        status.sitl_origin = null
        status.sitl_endpoint = null
        status.sitl_source = null
        status.sitl_feed_age_s = null
        return
      }
      if (command === 'actuator_set_motor_control') {
        status.motor_control.enabled = args.enabled
        if (!args.enabled) {
          test.cancelMotorStart()
          if (status.session === 'running' || status.session === 'stopping') {
            status.session = 'idle'
            status.output_values = []
            status.output_pwm = []
            test.emit('actuator_event', { kind: 'stopped', reason: 'motor control off' })
          }
        }
        return structuredClone(status.motor_control)
      }
      if (command === 'actuator_stream_catalog') {
        const query = args.query.trim().toUpperCase()
        const rank = (message) => message.name === query || String(message.id) === query ? 0 : message.px4_mapping?.uorb_sources.some((source) => source.toUpperCase() === query) ? 1 : 2
        return streamCatalog.filter((message) => rank(message) < 2 || message.name.includes(query) || message.fields.some((field) => field.name.toUpperCase().includes(query)))
          .sort((a, b) => rank(a) - rank(b) || a.id - b.id)
      }
      if (command === 'actuator_stream_status') {
        streams.target = status.target_connected ? { connection_id: test.connectionId, target_system_id: status.target_system_id, target_component_id: 1 } : null
        streams.ready = status.target_connected
        return structuredClone(streams)
      }
      if (command === 'actuator_stream_set_rate') {
        if (args.target.connection_id !== test.connectionId || args.target.target_system_id !== status.target_system_id || args.target.target_component_id !== 1) throw new Error('stream target changed')
        const complete = () => {
          const { request } = args
          const row = streamRow(request)
          streams.statuses = [...streams.statuses.filter((old) => old.message_id !== request.message_id), row].sort((a, b) => a.message_id - b.message_id)
          initialStreams.set(request.message_id, request)
          localStorage.setItem('test.backend.streams', JSON.stringify([...initialStreams.values()]))
          return structuredClone(row)
        }
        if (test.holdStreamWrite) return new Promise((resolve) => { test.pendingStreamWrite = () => resolve(complete()) })
        return complete()
      }
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
        if (!status.motor_control.enabled) throw new Error('Motor control is OFF')
        if (test.startError) throw new Error(test.startError)
        const complete = () => {
          status.session = 'running'
          status.output_values = [0.5, 0.5, 0.25, 0]
          status.output_pwm = [1500, 1500, 1750, null]
          test.emit('actuator_event', { kind: 'state', state: 'manual_running' })
        }
        if (test.holdMotorStart) {
          Object.assign(status.tones, { starting: true, busy: true, trigger: 'starting', phase: 'playing' })
          return new Promise((resolve, reject) => {
            test.pendingMotorStart = { cancelled: false, reject, complete: () => { complete(); resolve() } }
          })
        }
        return complete()
      }
      if (command === 'actuator_set_tones') {
        status.tones.settings = structuredClone(args.settings)
        return
      }
      if (command === 'actuator_stop') {
        test.cancelMotorStart()
        status.session = 'idle'
        status.output_values = []
        status.output_pwm = []
        test.emit('actuator_event', { kind: 'stopped', reason: 'command' })
        return
      }
      if (command === 'actuator_estop') {
        test.cancelMotorStart()
        status.session = 'emergency'
        status.output_values = []
        status.output_pwm = []
        test.emit('actuator_event', { kind: 'state', state: 'emergency' })
        test.emit('actuator_event', { kind: 'stopped', reason: 'emergency' })
        return
      }
      if (command === 'actuator_reset_emergency') {
        status.session = 'idle'
        return
      }
      if (command === 'actuator_refresh_mapping') {
        if (test.mappingError) throw new Error(test.mappingError)
        return
      }
      if (['actuator_capture_verify', 'actuator_capture_start', 'actuator_capture_stop'].includes(command)) {
        const wasRecording = capture.phase === 'recording'
        const complete = () => {
          if (command === 'actuator_capture_start') {
            capture.phase = 'recording'
            capture.remote_path = '/fs/microsd/log/2026-10-22/test.ulg'
          } else if (command === 'actuator_capture_stop') {
            capture.phase = 'complete'
          } else {
            capture.phase = wasRecording ? 'recording' : 'ready'
          }
          return structuredClone(capture)
        }
        if (test.holdCapture === command) {
          capture.phase = command === 'actuator_capture_stop' ? 'stopping' : 'verifying'
          return new Promise((resolve) => {
            test.pendingCapture = { command, complete: () => resolve(complete()) }
          })
        }
        return complete()
      }
      if (command === 'params_cache_get') return { value: { int: 0 } }
      if (command === 'config_load') return null
      return undefined
    },
  }
})

const calls = async (command) => page.evaluate((name) => window.__bench.calls.filter((call) => call.command === name).length, command)
const resetCalls = () => page.evaluate(() => { window.__bench.calls = [] })
const start = () => page.locator('button.start-btn')
const ensureUnlocked = async () => {
  const control = page.getByRole('switch', { name: 'Motor control', exact: true })
  if (await control.getAttribute('aria-checked') === 'false') await control.click()
  const unlock = page.getByRole('button', { name: 'Unlock test', exact: true })
  if ((await unlock.count()) > 0) await unlock.click()
}
/** Every start requires a 3 s hold; a short click must do nothing. */
const holdStart = async (ms = 3150) => {
  await start().scrollIntoViewIfNeeded()
  const bounds = await start().boundingBox()
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)
  await page.mouse.down()
  await page.waitForTimeout(ms)
  await page.mouse.up()
}

const waitForStreamViewport = () => page.waitForFunction(() => {
  const panel = document.querySelector('.stream-panel')?.getBoundingClientRect()
  const header = document.querySelector('.actuator-header')?.getBoundingClientRect()
  const drawer = document.querySelector('.logging-drawer')
  const bottom = drawer && getComputedStyle(drawer).position === 'fixed'
    ? drawer.getBoundingClientRect().top : window.innerHeight
  return panel && header && panel.height >= 360 && panel.top >= header.bottom
    && panel.bottom <= bottom && panel.left >= 0 && panel.right <= window.innerWidth
}).catch(async (error) => {
  console.error('Stream viewport bounds:', await page.evaluate(() => {
    const bounds = (selector) => {
      const element = document.querySelector(selector)
      if (!element) return null
      return { rect: element.getBoundingClientRect().toJSON(), scrollTop: element.scrollTop,
        scrollHeight: element.scrollHeight, clientHeight: element.clientHeight,
        overflowY: getComputedStyle(element).overflowY, position: getComputedStyle(element).position }
    }
    return { panel: bounds('.stream-panel'), mid: bounds('.actuator-mid'), scope: bounds('.scope-grid'),
      header: bounds('.actuator-header'), drawer: bounds('.logging-drawer') }
  }))
  throw error
})

const emitOutputCurves = (values) => page.evaluate((values) => {
  const now = Date.now() / 1000
  const samples = []
  for (let i = 0; i <= 100; i++) {
    const t = now - 10 + i / 10
    const offset = 20 * Math.sin(i * Math.PI * 2 / 100)
    for (const [id, value] of Object.entries(values)) samples.push({ id, t, v: value + offset })
  }
  window.__bench.emit('actuator_samples', samples)
}, values)

// A rebuilt uPlot starts empty. Wait for real trace pixels, not only its legend
// or the independently refreshed signal list, before accepting a plot capture.
const waitForOutputPlot = () => page.waitForFunction(() => {
  const pane = document.querySelector('.scope-pane[data-pane="top"]')
  const canvas = pane?.querySelector('canvas')
  if (!canvas?.width || !canvas.height) return false
  const context = canvas.getContext('2d')
  if (!context) return false
  const plot = pane.querySelector('.scope-pane-plot')?.__maggcsScopePlot
  if (!plot) return false
  const probe = document.createElement('canvas')
  probe.width = probe.height = 1
  const probeContext = probe.getContext('2d')
  const colors = plot.series.slice(1).map((series, index) => {
    const stroke = typeof series.stroke === 'function' ? series.stroke(plot, index + 1) : series.stroke
    probeContext.fillStyle = stroke
    probeContext.fillRect(0, 0, 1, 1)
    return [...probeContext.getImageData(0, 0, 1, 1).data].slice(0, 3)
  })
  const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data
  let painted = 0
  for (let i = 0; i < pixels.length; i += 4) {
    if (pixels[i + 3] > 0 && colors.some((color) =>
      color.every((channel, offset) => Math.abs(channel - pixels[i + offset]) <= 2),
    )) painted += 1
    if (painted >= 20) return true
  }
  return false
}, null, { timeout: 5000 }).catch(async (error) => {
  console.error('Output plot diagnostics:', await page.evaluate(() => {
    const host = document.querySelector('.scope-pane[data-pane="top"] .scope-pane-plot')
    const plot = host?.__maggcsScopePlot
    return {
      host: host?.getBoundingClientRect().toJSON(), width: plot?.width, height: plot?.height,
      bbox: plot?.bbox, scales: plot?.scales,
      series: plot?.series.map((series, index) => ({
        label: series.label, show: series.show, scale: series.scale,
        values: plot.data[index]?.filter(Number.isFinite).length,
      })),
    }
  }))
  throw error
})

const outputPlotGapState = () => page.evaluate(() => {
  const pane = document.querySelector('.scope-pane[data-pane="top"]')
  const host = pane?.querySelector('.scope-pane-plot')
  const plot = host?.__maggcsScopePlot
  if (!plot) return { mounted: false, spanGaps: null, nulls: 0, before: 0, after: 0 }
  const seriesIndex = plot.series.findIndex((series) => series.label === 'Command PWM M 1')
  if (seriesIndex < 1) return { mounted: true, spanGaps: null, nulls: 0, before: 0, after: 0 }
  const values = [...plot.data[seriesIndex]]
  const gapIndex = values.findIndex((value, index) => value === null
    && values.slice(0, index).some(Number.isFinite)
    && values.slice(index + 1).some(Number.isFinite))
  return {
    mounted: true,
    spanGaps: plot.series[seriesIndex].spanGaps,
    nulls: values.filter((value) => value === null).length,
    before: gapIndex < 0 ? 0 : values.slice(0, gapIndex).filter(Number.isFinite).length,
    after: gapIndex < 0 ? 0 : values.slice(gapIndex + 1).filter(Number.isFinite).length,
  }
})

const waitForOutputGapData = () => page.waitForFunction(() => {
  const pane = document.querySelector('.scope-pane[data-pane="top"]')
  const plot = pane?.querySelector('.scope-pane-plot')?.__maggcsScopePlot
  if (!plot) return false
  const seriesIndex = plot.series.findIndex((series) => series.label === 'Command PWM M 1')
  if (seriesIndex < 1) return false
  const values = [...plot.data[seriesIndex]]
  return values.some((value, index) => value === null
    && values.slice(0, index).filter(Number.isFinite).length > 1
    && values.slice(index + 1).filter(Number.isFinite).length > 1)
})

const outputAxisRange = () => page.evaluate(() => {
  const pane = document.querySelector('.scope-pane[data-pane="top"]')
  const plot = pane?.querySelector('.scope-pane-plot')?.__maggcsScopePlot
  return plot ? { min: plot.scales.x.min, max: plot.scales.x.max } : null
})

const emitSeparatedCommandSegments = (gap) => page.evaluate(({ gap }) => {
  const now = Date.now() / 1000
  const samples = [{ id: 'session.running', t: now - 28, v: 1 }]
  for (let i = 0; i <= 20; i++) {
    samples.push({ id: 'cmd.pwm1', t: now - 27 + i * 0.1, v: 1450 + i })
  }
  if (gap === 'released') {
    samples.push({ id: 'cmd.pwm1', t: now - 15, v: null, gap: 'released' })
  } else {
    samples.push({ id: 'session.running', t: now - 15, v: 0 })
    samples.push({ id: 'session.running', t: now - 8, v: 1 })
  }
  for (let i = 0; i <= 20; i++) {
    samples.push({ id: 'cmd.pwm1', t: now - 7 + i * 0.1, v: 1550 + i })
  }
  window.__bench.emit('actuator_samples', samples)
}, { gap })

try {
  if (shots) await mkdir(shots, { recursive: true })
  await page.goto(url, { waitUntil: 'networkidle' })
  const motorControl = page.getByRole('switch', { name: 'Motor control', exact: true })
  assert.equal(await motorControl.getAttribute('aria-checked'), 'false', 'new window starts monitor-only')
  assert.equal(await start().isDisabled(), true)
  const guide = page.getByTestId('motor-guide')
  assert.equal(await guide.getAttribute('data-action'), 'enable', 'the guide names the next safe action')
  assert.match(await guide.textContent(), /Enable motor control/)
  assert.equal(await motorControl.evaluate((el) => el.classList.contains('guide-target')), true, 'the matching control pulses')
  assert.equal(await page.getByRole('button', { name: 'Unlock test', exact: true }).isDisabled(), true)
  assert.match(await start().getAttribute('title'), /Motor control is OFF/)
  assert.equal(await calls('actuator_start_manual'), 0)
  assert.equal(await calls('params_cache_get'), 0, 'FC parameters never come from an unrelated primary cache')
  assert.equal(await page.getByText('Primary GCS', { exact: true }).count(), 0, 'no second FC indicator')
  assert.equal(await page.evaluate(() => window.__bench.mainConnected), false, 'dedicated target needs no main FC')
  assert.match(await page.locator('.actuator-header').textContent(), /DISARMED/)
  assert.match(await page.locator('.controls-grid').textContent(), /full PX4-configured output range \(0–100%\)/)
  if (shots && !fcRangeOnly) await page.screenshot({ path: join(shots, 'motor-controls-control-off-1366x768.png') })
  if (fcRangeOnly) {
    await page.evaluate(() => { window.__bench.status.com_mot_test_en = null })
    await page.waitForFunction(() => document.querySelector('.mot-en-light')?.textContent.includes('unknown'))
    assert.equal(await page.locator('.mot-en-gate').count(), 0, 'unknown target permission never falls back to the primary cache')
    await page.evaluate(() => { window.__bench.status.com_mot_test_en = 0 })
    await page.locator('.mot-en-gate').waitFor()
    assert.equal(await page.getByRole('button', { name: 'Open Parameters', exact: true }).count(), 0, 'dedicated FC never opens another FC’s parameter editor')
    await page.evaluate(() => { window.__bench.status.com_mot_test_en = 1 })
    await page.locator('.mot-en-gate').waitFor({ state: 'detached' })
    await ensureUnlocked()
    const slider = page.locator('.motor-control-row input[type="range"]').first()
    assert.equal(await slider.getAttribute('max'), '1', 'the control spans the full normalized range')
    await slider.focus()
    await page.keyboard.press('End')
    assert.equal(await slider.inputValue(), '1')
    await ensureUnlocked()
    await holdStart()
    await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'STOP')
    const request = await page.evaluate(() => window.__bench.calls.find((call) => call.command === 'actuator_start_manual'))
    assert.deepEqual(request.args.values, [1, 1, 1, 1], '100% reaches the command API without a 30% cap')
    await page.evaluate(() => {
      const test = window.__bench
      Object.assign(test.status, {
        output_values: [1, 1, 0, 1], output_pwm: [1900, 2000, 2000, 2000],
        fc_output_pwm: [1900, 2000, 2000, 2000],
      })
      const now = Date.now() / 1000
      const samples = []
      for (let i = 0; i <= 150; i++) {
        const t = now - 15 + i / 10
        const v = 1100 + 800 * 0.5 * (1 - Math.cos(i * Math.PI * 2 / 100))
        samples.push({ id: 'fc.motor1', t, v })
        samples.push(i % 100 === 0
          ? { id: 'cmd.pwm1', t, v: null, gap: 'released' }
          : { id: 'cmd.pwm1', t, v })
      }
      test.emit('actuator_samples', samples)
    })
    await page.waitForFunction(() => document.querySelector('[data-testid="motor-command-pwm-1"]')?.textContent === '1900 µs')
    await waitForOutputPlot()
    const peak = await page.evaluate(() => {
      const plot = document.querySelector('.scope-pane[data-pane="top"] .scope-pane-plot').__maggcsScopePlot
      const index = plot.series.findIndex((series) => series.label === 'Command PWM M 1')
      return { value: Math.max(...plot.data[index].filter(Number.isFinite)), axisMax: plot.scales['µs'].max }
    })
    assert(Math.abs(peak.value - 1900) < 1, 'the displayed sine reaches the configured 1900 µs maximum')
    assert(peak.axisMax >= 1900, 'the chart displays the full output range')
    if (shots) {
      await page.setViewportSize({ width: 1920, height: 1080 })
      await waitForOutputPlot()
      await page.screenshot({ path: join(shots, 'motor-dedicated-fc-full-range-1920x1080.png') })
      await page.setViewportSize({ width: 1366, height: 768 })
    }
    await page.locator('.actuator-header .stop-tab').click()
    await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'START')
    await motorControl.click()
    await resetCalls()
    console.log('PASS: dedicated target alone supplies FC status; target-only permission; 100% command and uncropped 1900 µs plot')
  }
  await page.evaluate(() => Object.assign(window.__bench.status, { connected: false, fc_alive: false, endpoint: null, target_connected: false, target_fc_alive: false, target_origin: null }))
  const connectPx4 = page.getByRole('button', { name: 'Connect PX4', exact: true })
  await connectPx4.waitFor()
  await page.waitForFunction(() => document.querySelector('[data-testid="motor-guide"]')?.dataset.action === 'connect')
  assert.match(await guide.textContent(), /Connect the serial PX4/)
  assert.equal(await connectPx4.evaluate((el) => el.classList.contains('guide-target')), true, 'the connect action pulses')
  assert.equal(await page.getByLabel('Target MAVLink system ID').count(), 0, 'the PX4 ID is discovered, not typed')
  assert.equal(await page.getByRole('button', { name: 'Use main serial link', exact: true }).count(), 0, 'UDP/unavailable main links keep the dedicated path')
  const mainOffer = { target: { connection_id: 1, target_system_id: 250, target_component_id: 1 }, transport_generation: 3, endpoint: 'serial:/dev/ttyACM0:115200' }
  await page.evaluate((offer) => {
    window.__bench.mainConnected = true
    window.__bench.status.main_serial_target = offer
  }, mainOffer)
  const useMain = page.getByRole('button', { name: 'Use main serial link', exact: true })
  await useMain.waitFor()
  if (shots && !fcRangeOnly) {
    await useMain.scrollIntoViewIfNeeded()
    await page.screenshot({ path: join(shots, 'motor-main-serial-offer-1366x768.png') })
  }
  await useMain.click()
  await page.getByTestId('main-serial-attached').waitFor()
  assert.equal(await calls('actuator_connect_target'), 0, 'borrowing never invokes a second connection')
  assert.deepEqual(await page.evaluate(() => window.__bench.calls.find((call) => call.command === 'actuator_use_main_target').args), { expected: mainOffer })
  assert.equal(await motorControl.getAttribute('aria-checked'), 'false', 'attach starts monitor-only')
  const detachMain = page.getByRole('button', { name: 'Detach main link', exact: true })
  assert.match(await detachMain.getAttribute('title'), /keep the main serial connection open/)
  assert.match(await page.locator('.actuator-header').textContent(), /DISARMED/)
  assert.equal(await calls('params_cache_get'), 0, 'borrowed FC permission also comes through the selected target status')
  if (fcRangeOnly) {
    await page.evaluate(() => { window.__bench.status.com_mot_test_en = 0 })
    await page.getByRole('button', { name: 'Open Parameters', exact: true }).waitFor()
    await page.evaluate(() => { window.__bench.status.com_mot_test_en = 1 })
    await page.locator('.mot-en-gate').waitFor({ state: 'detached' })
  }
  if (shots) await page.screenshot({ path: join(shots, fcRangeOnly ? 'motor-borrowed-main-fc-1366x768.png' : 'motor-main-serial-attached-1366x768.png') })
  await detachMain.click()
  await useMain.waitFor()
  assert.equal(await page.evaluate(() => window.__bench.mainConnected), true, 'detaching preserves the main FC connection')
  await page.evaluate(() => { window.__bench.mainLinkError = 'main serial target changed — refresh the target offer' })
  await useMain.click()
  await page.getByText(/main serial target changed — refresh/).waitFor()
  assert.equal(await calls('actuator_start_manual'), 0)
  await page.evaluate(() => Object.assign(window.__bench.status, { main_serial_target: null }))
  await page.waitForFunction(() => ![...document.querySelectorAll('button')].some((button) => button.textContent === 'Use main serial link'))
  console.log('PASS: main serial offer, exact generation IPC, no second open, passive attach, detach and stale-offer refusal')
  if (process.env.MOTOR_TARGET_ONLY === '1' || fcRangeOnly) {
    assert.deepEqual(errors, [])
    await browser.close()
    process.exit(0)
  }
  await page.reload({ waitUntil: 'networkidle' })
  await motorControl.click()
  assert.equal(await motorControl.getAttribute('aria-checked'), 'true')
  assert.equal(await calls('actuator_start_manual'), 0, 'enabling control never starts motors')
  assert.equal(await page.locator('.actuator-left button.start-btn').count(), 1, 'one start control in the left sidebar')
  assert.equal(await page.locator('.actuator-left [data-testid="motor-mag-alarm"]').count(), 1, 'magnetic alarm shares the left sidebar')
  assert.equal(await page.getByTestId('motor-session-state').textContent(), 'LOCKED', 'one explicit locked state in the header')
  assert.equal(await page.getByTestId('motor-active-protocol').textContent(), 'OutputPWM 400 Hz', 'active protocol is fixed in the header')
  assert.equal(await page.locator('.actuator-header .header-chip', { hasText: 'FW' }).count(), 0, 'unavailable firmware placeholder is removed')
  assert.equal(await page.locator('.actuator-header .header-chip', { hasText: 'Rate' }).count(), 0, 'unavailable measured-rate placeholder is removed')
  assert.equal(await page.locator('.actuator-header .header-chip', { hasText: 'SD' }).count(), 0, 'capture state no longer crowds the safety header')
  assert.equal(await page.locator('.state-strip').count(), 0, 'no contradictory duplicate state strip')
  const trackingFold = page.getByTestId('motor-tracking-fold')
  assert.equal(await trackingFold.textContent(), 'Unfold')
  assert.equal(await page.locator('#motor-tracking-body').isVisible(), false, 'healthy idle tracking can fold')
  const sourceDetails = page.getByRole('button', { name: 'SITL source details', exact: true })
  assert.equal(await sourceDetails.getAttribute('aria-expanded'), 'false', 'inactive SITL configuration starts collapsed')
  const outputFold = page.locator('.fold-head', { hasText: 'Connection & output details' })
  assert.equal(
    await outputFold.getAttribute('aria-expanded'),
    'false',
    'secondary output details start folded when the configuration is healthy',
  )
  await page.getByRole('button', { name: 'Forward', exact: true }).click()
  await page.waitForFunction(() => document.getElementById('motor-sitl-details')?.hidden === false)
  // Main-link SITL feed: the offer mirrors the main serial-target pattern.
  // Borrowing must reuse the main connection, never open a second UDP link.
  const mainSitlOffer = {
    endpoint: 'udpin:0.0.0.0:14550',
    system_id: 1,
    component_id: 1,
    transport_generation: 3,
  }
  await page.evaluate((offer) => { window.__bench.status.main_sitl_source = offer }, mainSitlOffer)
  const useMainSitl = page.getByRole('button', { name: 'Use main link', exact: true })
  await useMainSitl.waitFor()
  await useMainSitl.click()
  await page.getByTestId('main-sitl-attached').waitFor()
  await page.getByText('connected · main link', { exact: true }).waitFor()
  assert.equal(await calls('actuator_connect_sitl'), 0, 'borrowing never opens a second SITL link')
  assert.deepEqual(
    await page.evaluate(() => window.__bench.calls.find((call) => call.command === 'actuator_use_main_sitl').args),
    { expected: mainSitlOffer, port: 'main' },
  )
  await page.getByRole('button', { name: 'Disconnect SITL', exact: true }).click()
  await useMainSitl.waitFor()
  await page.evaluate(() => { window.__bench.status.main_sitl_source = null })
  await page.waitForFunction(() => ![...document.querySelectorAll('button')].some((button) => button.textContent === 'Use main link'))
  await page.getByRole('button', { name: 'Manual', exact: true }).click()
  await page.waitForFunction(() => document.getElementById('motor-sitl-details')?.hidden === true)
  assert.equal(await calls('actuator_start_forward'), 0, 'changing the displayed source never starts a test')
  await page.getByRole('button', { name: 'Unlock test', exact: true }).click()
  const fcModels = await page.evaluate(() => structuredClone(window.__bench.status.pwm_models))
  await page.evaluate(() => { window.__bench.status.pwm_models = [] })
  await page.waitForFunction(() => document.querySelector('button.start-btn')?.disabled === true)
  await page.waitForFunction(() => document.querySelector('[data-testid="motor-tracking-fold"]')?.textContent === 'required')
  assert.equal(await trackingFold.isDisabled(), true, 'configuration warnings lock tracking open')
  assert.equal(await page.locator('#motor-tracking-body').isVisible(), true)
  assert.match(await page.getByTestId('motor-output-warning').textContent(), /models are not loaded/i)
  assert.equal(await page.getByTestId('motor-range-1').textContent(), 'Not read', 'missing models are not presented as FC ranges')
  await page.evaluate((models) => { window.__bench.status.pwm_models = models }, fcModels)
  await page.waitForFunction(() => document.querySelector('button.start-btn')?.disabled === false)
  await page.waitForFunction(() => document.querySelector('[data-testid="motor-tracking-fold"]')?.textContent === 'Unfold')
  assert.equal(await page.locator('#motor-tracking-body').isVisible(), false, 'temporary warning restores the idle fold state')
  await trackingFold.click()
  await page.getByRole('button', { name: 'Motor 1 details', exact: true }).click()
  assert.equal(await page.locator('.motor-detail').count(), 1, 'operator-pinned motor detail opens')
  await ensureUnlocked()
  await resetCalls()
  await start().click({ delay: 20 })
  await page.waitForTimeout(200)
  assert.equal(await calls('actuator_start_manual'), 0, 'a short click never starts')
  await holdStart()
  await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'STOP')
  assert.equal(await calls('actuator_start_manual'), 1, 'one 3 s hold starts once')
  assert.equal(await calls('actuator_stop'), 0, 'release after the hold never clicks STOP')
  assert.equal(await start().textContent(), 'STOP')
  assert.equal(await trackingFold.textContent(), 'required')
  assert.equal(await trackingFold.isDisabled(), true, 'running tracking cannot be folded')
  assert.equal(await page.locator('#motor-tracking-body').isVisible(), true)

  await page.getByText('tracking', { exact: true }).first().waitFor()
  const rows = page.locator('.motor-table tbody > tr:not(.motor-detail)')
  assert.equal(await page.getByTestId('motor-range-1').textContent(), '1100–1900 µs', 'the FC-read range is visible without expanding details')
  assert.equal(await rows.nth(0).locator('td').nth(2).textContent(), 'PWM 400 Hz', 'the PX4 output protocol is explicit')
  assert.equal(await page.getByTestId('motor-command-pwm-1').textContent(), '1500 µs', 'M1 converted pulse from the sender')
  assert.equal(await rows.nth(0).locator('td').nth(7).textContent(), '0 µs', 'M1 tracks')
  assert.equal(await page.getByTestId('motor-command-pwm-3').textContent(), '1750 µs', 'M3 inverted model expected pulse')
  assert.equal(await rows.nth(2).locator('td').nth(8).textContent(), 'mismatch', 'M3 outside ±5 µs')
  assert.equal(await rows.nth(3).locator('td').nth(8).textContent(), 'released', 'M4 has no active PWM command')
  await page.locator('.actuator-header .stop-tab').click()
  await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'START')
  assert.equal(await calls('actuator_stop'), 1)
  assert.equal(await trackingFold.isDisabled(), false)
  assert.equal(await page.locator('.motor-detail').count(), 1, 'pinned detail survives the temporary running requirement')
  await ensureUnlocked()
  await holdStart()
  await page.waitForFunction(() => document.querySelector('[data-testid="motor-session-state"]')?.textContent === 'RUNNING')
  await motorControl.click()
  await page.waitForFunction(() => window.__bench.status.session === 'idle' && !window.__bench.status.motor_control.enabled)
  assert.equal(await start().isDisabled(), true, 'OFF leaves no ready-to-start control')
  assert.equal(await page.locator('.motor-control-row input').first().isDisabled(), true)
  await motorControl.hover()
  assert.match(await motorControl.getAttribute('title'), /does not start motors/)
  assert.match(await page.locator('.actuator-header .estop-btn').getAttribute('title'), /owned outputs/)
  await motorControl.click()

  await page.evaluate(() => { window.__bench.status.target_fc_alive = false })
  await page.waitForFunction(() => document.querySelector('.actuator-header')?.textContent?.includes('HEARTBEAT LOST'))
  assert.equal(await trackingFold.isDisabled(), true, 'stale target feedback locks tracking open')
  await page.evaluate(() => { window.__bench.status.target_fc_alive = true })
  await page.waitForFunction(() => document.querySelector('.actuator-header')?.textContent?.includes('DISARMED'))

  await page.evaluate(() => { window.__bench.mappingError = 'simulated mapping failure' })
  if ((await outputFold.getAttribute('aria-expanded')) === 'false') await outputFold.click()
  await page.getByRole('button', { name: 'Refresh output configuration', exact: true }).click()
  await page.getByText(/simulated mapping failure/).waitFor()
  await page.getByRole('button', { name: 'Dismiss', exact: true }).click()
  assert.equal(await page.getByText(/simulated mapping failure/).count(), 0, 'mapping error dismisses')
  await page.evaluate(() => { window.__bench.mappingError = null })
  await outputFold.click()
  await page.getByRole('button', { name: 'Unlock test', exact: true }).click()

  await page.evaluate(() => { window.__bench.startError = 'simulated start rejection' })
  await ensureUnlocked()
  await resetCalls()
  await holdStart()
  await page.getByText(/start failed: Error: simulated start rejection/).waitFor()
  assert.equal(await calls('actuator_start_manual'), 1)
  await page.getByRole('button', { name: 'Dismiss', exact: true }).click()
  await page.evaluate(() => { window.__bench.startError = null })

  await page.getByRole('button', { name: 'Preset', exact: true }).click()
  await page.getByRole('button', { name: 'Unlock test', exact: true }).click()
  await page.getByRole('button', { name: 'Sine', exact: true }).click()
  await page.getByRole('button', { name: 'Unlock test', exact: true }).waitFor()
  await page.getByRole('button', { name: 'Manual', exact: true }).click()

  await ensureUnlocked()
  await resetCalls()

  const loggingFold = page.getByRole('button', { name: /Capture & logs/ })
  assert.equal(
    await loggingFold.getAttribute('aria-expanded'),
    'false',
    'logging drawer starts closed',
  )
  await loggingFold.click()
  assert.equal(await loggingFold.getAttribute('aria-expanded'), 'true')
  const motorControls = page.locator('.motor-control-row')
  const motorBoxes = await motorControls.evaluateAll((rows) => rows.map((row) => {
    const box = row.getBoundingClientRect()
    const slider = row.querySelector('input[type="range"]').getBoundingClientRect()
    return { x: box.x, y: box.y, width: box.width, sliderWidth: slider.width }
  }))
  assert.equal(motorBoxes.length, 4)
  assert(Math.abs(motorBoxes[0].y - motorBoxes[1].y) < 2, 'M1/M2 share the first row')
  assert(Math.abs(motorBoxes[2].y - motorBoxes[3].y) < 2, 'M3/M4 share the second row')
  assert(motorBoxes[1].x > motorBoxes[0].x && motorBoxes[3].x > motorBoxes[2].x, 'motor controls use two columns')
  assert(motorBoxes.every((box) => box.sliderWidth <= 260), 'desktop motor sliders stay compact')
  const drawerBox = await page.locator('.logging-drawer').boundingBox()
  assert(drawerBox && drawerBox.x >= 236 && drawerBox.x + drawerBox.width > 1300, 'logging drawer spans center and right workspace')
  assert.equal(await page.locator('.target-card > .row').count(), 3, 'healthy target summary stays concise')
  if (shots) {
    await page.setViewportSize({ width: 1366, height: 768 })
    await page.screenshot({ path: join(shots, 'motor-controls-logging-drawer-1366x768.png') })
  }

  await ensureUnlocked()
  await resetCalls()
  await holdStart()
  await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'STOP')
  assert.equal(await calls('actuator_capture_start'), 0, 'motor START does not begin manual capture')
  await page.getByRole('button', { name: 'Start capture', exact: true }).click()
  await page.waitForTimeout(100)
  assert.equal(await calls('actuator_capture_start'), 1, 'capture can start while the motor test runs')
  await page.getByRole('button', { name: 'Stop capture', exact: true }).click()
  await page.waitForTimeout(100)
  assert.equal(await calls('actuator_capture_stop'), 1, 'capture can stop while the motor test runs')
  await start().click()
  await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'START')
  assert.equal(await calls('actuator_capture_stop'), 1, 'motor STOP does not stop manual capture')

  await resetCalls()
  await page.getByRole('button', { name: 'Start capture', exact: true }).click()
  await ensureUnlocked()
  await holdStart()
  await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'STOP')
  await start().click()
  await page.waitForFunction(() => document.querySelector('button.start-btn')?.textContent === 'START')
  assert.equal(await calls('actuator_capture_stop'), 0, 'motor STOP does not stop an active manual capture')
  await page.getByRole('button', { name: 'Stop capture', exact: true }).click()

  // MR-16: keep the capture IPC reply unresolved until after the user has
  // stopped the motor session. A late reply must not restart or unlatch it.
  for (const operation of [
    { command: 'actuator_capture_start', button: 'Start capture' },
    { command: 'actuator_capture_stop', button: 'Stop capture' },
    { command: 'actuator_capture_verify', button: 'Check logger' },
  ]) {
    for (const emergency of [false, true]) {
      await page.evaluate((command) => {
        window.__bench.status.sd_capture.phase = command === 'actuator_capture_stop' ? 'recording' : 'idle'
      }, operation.command)
      const captureButton = page.getByRole('button', { name: operation.button, exact: true })
      await page.waitForFunction((label) => [...document.querySelectorAll('button')]
        .some((button) => button.textContent.trim() === label && !button.disabled), operation.button)
      await ensureUnlocked()
      await holdStart()
      await page.waitForFunction(() => document.querySelector('[data-testid="motor-session-state"]')?.textContent === 'RUNNING')
      await resetCalls()
      await page.evaluate((command) => { window.__bench.holdCapture = command }, operation.command)
      await captureButton.click()
      await page.waitForFunction((command) => window.__bench.pendingCapture?.command === command, operation.command)
      assert.equal(await captureButton.isDisabled(), true, 'capture action waits for its own reply')
      const stopButton = page.locator('.actuator-header .stop-tab')
      const emergencyButton = page.locator('.actuator-header .estop-btn')
      assert.equal(await stopButton.isEnabled(), true, 'STOP stays enabled during capture IPC')
      assert.equal(await emergencyButton.isEnabled(), true, 'E-STOP stays enabled during capture IPC')
      if (emergency && operation.command === 'actuator_capture_verify') {
        await page.locator('.motor-control-row input[type="range"]').first().focus()
        await page.keyboard.press('Escape')
      } else {
        await (emergency ? emergencyButton : stopButton).click()
      }
      await page.waitForFunction((emergency) => {
        const session = window.__bench.status.session
        const stopped = document.querySelector('.actuator-header .stop-tab')?.disabled === true
        const label = document.querySelector('[data-testid="motor-session-state"]')?.textContent
        return session === (emergency ? 'emergency' : 'idle') && stopped
          && (emergency ? label === 'EMERGENCY' : label === 'LOCKED' || label === 'UNLOCKED')
      }, emergency)
      const stoppedState = await page.getByTestId('motor-session-state').textContent()
      assert.equal(await calls(emergency ? 'actuator_estop' : 'actuator_stop'), 1, 'one stop command is dispatched promptly')
      assert.equal(await page.evaluate(() => window.__bench.pendingCapture !== null), true, 'stop finishes before capture replies')
      assert.equal(await calls('actuator_run_abort'), 0, 'manual stop does not enter the coordinated run path')
      if (shots && operation.command === 'actuator_capture_start') {
        await page.screenshot({ path: join(shots, `motor-controls-capture-pending-${emergency ? 'estop' : 'stop'}-1366x768.png`) })
      }
      await page.evaluate(() => window.__bench.completeCapture())
      await page.waitForFunction(() => [...document.querySelectorAll('button')]
        .some((button) => button.textContent.trim() === 'Check logger' && !button.disabled))
      assert.equal(await page.getByTestId('motor-session-state').textContent(), stoppedState, 'late capture reply preserves the stopped state')
      assert.equal(await calls('actuator_start_manual'), 0, 'late capture reply never starts motors')
      assert.equal(await calls(operation.command), 1, 'capture is not retried by the stop action')
      if (emergency) {
        await page.getByRole('button', { name: 'RESET', exact: true }).click()
        await page.waitForFunction(() => document.querySelector('[data-testid="motor-session-state"]')?.textContent === 'LOCKED')
      }
    }
  }
  console.log('PASS: STOP/E-STOP preempt all three pending capture operations; late replies preserve stop/latch state')

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
  await loggingFold.click()
  assert.equal(await loggingFold.getAttribute('aria-expanded'), 'false', 'logging drawer closes without hiding capture actions')

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

  // Deterministic traces cover all signal categories in two independent plots.
  await page.evaluate(() => {
    const test = window.__bench
    test.emit('actuator_event', { kind: 'stopped', reason: 'simulated IPC fixture' })
    const now = Date.now() / 1000
    const samples = []
    for (let i = 0; i <= 200; i++) {
      const t = now - 20 + i / 10
      samples.push(
        { id: 'fc.motor1', t, v: 1500 + 20 * Math.sin(i / 10) },
        { id: 'cmd.pwm1', t, v: 1500 + 20 * Math.sin(i / 10) },
        { id: 'mag.raw_total', t, v: 50000 + 50 * Math.sin(i / 8) },
        { id: 'esc1.rpm', t, v: 4000 + 300 * Math.sin(i / 12) },
        { id: 'bat1.current', t, v: 12 + 2 * Math.sin(i / 6) },
      )
    }
    test.emit('actuator_samples', samples)
  })
  await page.waitForFunction(() => document.querySelectorAll('.scope-pane').length === 2)
  assert.equal(
    await page.locator('.signal-row', { hasText: 'Speed target' }).count(),
    0,
    'no speed-response signals are offered',
  )
  const pwmTrace = page.locator('.signal-row', { hasText: 'Command PWM M 1' })
  await pwmTrace.waitFor()
  assert.ok((await pwmTrace.locator('.signal-value').innerText()).includes('µs'), 'command plot uses PWM units')
  const commandPane = page.locator('.scope-pane[data-pane="top"]')
  const bottomPane = page.locator('.scope-pane[data-pane="bottom"]')
  await commandPane.locator('.u-label', { hasText: 'Command PWM M 1' }).first().waitFor()
  assert.equal(await commandPane.locator('.u-label', { hasText: 'Command PWM M 1' }).count(), 1, 'one PWM legend entry per signal ID')
  assert.equal(await commandPane.locator('.u-label', { hasText: 'FC output M 1 (MAIN1)' }).count(), 1, 'feedback is labelled with its mapped physical output')
  const colorPicker = page.getByLabel('Color for Command PWM M 1', { exact: true })
  const commandsBeforeColor = await calls('actuator_set_values')
  const canvasBeforeColor = await commandPane.locator('canvas').first().elementHandle()
  await colorPicker.fill('#ff00aa')
  await page.waitForFunction((canvas) => !canvas.isConnected, canvasBeforeColor)
  assert.equal(await colorPicker.inputValue(), '#ff00aa')
  assert.equal(
    await page.evaluate(() => JSON.parse(localStorage.getItem('maggcs.actuator.scope')).colors['cmd.pwm1']),
    '#ff00aa',
    'custom trace color persists by stable signal ID',
  )
  assert.equal(await calls('actuator_set_values'), commandsBeforeColor, 'changing trace color sends no motor command')
  await page.evaluate(() => window.__bench.emit('actuator_samples', [
    { id: 'cmd.pwm1', t: Date.now() / 1000, v: null, gap: 'released' },
  ]))
  await page.waitForFunction(() => [...document.querySelectorAll('.signal-row')].some((row) =>
    row.textContent.includes('Command PWM M 1') && row.querySelector('.state-dot.released'),
  ))
  assert.equal(await pwmTrace.locator('.signal-value').innerText(), '—', 'release leaves a gap, not a made-up 0 or minimum PWM')

  const clearScope = page.getByRole('button', { name: 'Clear', exact: true })
  await clearScope.click()
  await emitSeparatedCommandSegments('released')
  await waitForOutputGapData()
  let renderedGap = await outputPlotGapState()
  if (shots) {
    await mkdir(shots, { recursive: true })
    await page.setViewportSize({ width: 1366, height: 768 })
    await commandPane.scrollIntoViewIfNeeded()
    await page.screenshot({ path: join(shots, 'motor-controls-scope-gaps-1366x768.png') })
  }
  assert.equal(renderedGap.mounted, true)
  assert.equal(renderedGap.spanGaps, false, 'the mounted command series cannot span gaps')
  assert(renderedGap.nulls > 0 && renderedGap.before > 1 && renderedGap.after > 1, 'mounted uPlot data keeps both segments around the release gap')
  await clearScope.click()
  await emitSeparatedCommandSegments('run_boundary')
  await waitForOutputGapData()
  renderedGap = await outputPlotGapState()
  assert.equal(renderedGap.mounted, true)
  assert.equal(renderedGap.spanGaps, false, 'the mounted command series cannot span run boundaries')
  assert(renderedGap.nulls > 0 && renderedGap.before > 1 && renderedGap.after > 1, 'mounted uPlot data keeps separate runs around a null boundary')

  await clearScope.click()
  await page.evaluate(() => window.__bench.emit('actuator_samples', [
    { id: 'esc1.rpm', t: Date.now() / 1000 - 1, v: 5000 },
  ]))
  await page.waitForFunction(() => [...document.querySelectorAll('.signal-row')].some((row) =>
    row.querySelector('.signal-name')?.textContent === 'ESC rpm 1' && row.querySelector('.state-dot.stale'),
  ))
  await page.evaluate(() => window.__bench.emit('actuator_samples', [
    { id: 'esc1.rpm', t: Date.now() / 1000, v: 5100 },
  ]))
  await page.waitForFunction(() => [...document.querySelectorAll('.signal-row')].some((row) =>
    row.querySelector('.signal-name')?.textContent === 'ESC rpm 1'
      && row.querySelector('.state-dot.live')
      && row.querySelector('.signal-value')?.textContent === '5100 rpm',
  ))
  await clearScope.click()
  await page.evaluate(() => {
    const now = Date.now() / 1000
    window.__bench.emit('actuator_samples', [
      { id: 'cmd.pwm1', t: now - 100, v: 1400 },
      { id: 'cmd.pwm1', t: now - 0.1, v: 1500 },
      { id: 'cmd.pwm1', t: now, v: 1510 },
    ])
  })
  await page.waitForFunction(() => {
    const pane = document.querySelector('.scope-pane[data-pane="top"]')
    const scale = pane?.querySelector('.scope-pane-plot')?.__maggcsScopePlot?.scales.x
    return scale?.min >= 69 && scale?.max >= 99
  })
  let timeRange = await outputAxisRange()
  assert(timeRange && timeRange.min >= 69 && timeRange.max >= 99, 'rolling axis shows cumulative elapsed seconds')
  await clearScope.click()
  await page.evaluate(() => {
    const now = Date.now() / 1000
    window.__bench.emit('actuator_samples', [
      { id: 'cmd.pwm1', t: now - 0.1, v: 1500 },
      { id: 'cmd.pwm1', t: now, v: 1510 },
    ])
  })
  await page.waitForFunction(() => {
    const pane = document.querySelector('.scope-pane[data-pane="top"]')
    const scale = pane?.querySelector('.scope-pane-plot')?.__maggcsScopePlot?.scales.x
    return scale?.min === 0 && scale?.max === 30
  })
  timeRange = await outputAxisRange()
  assert.deepEqual(timeRange, { min: 0, max: 30 }, 'Clear resets elapsed plotting time to 0 s')
  await page.evaluate(() => {
    const now = Date.now() / 1000
    window.__bench.emit('actuator_samples', [
      { id: 'cmd.pwm1', t: now - 0.1, v: 1500 },
      { id: 'cmd.pwm1', t: now, v: 1510 },
      { id: 'fc.motor1', t: now - 0.1, v: 1498 },
      { id: 'fc.motor1', t: now, v: 1508 },
      { id: 'mag.raw_total', t: now - 0.1, v: 50000 },
      { id: 'mag.raw_total', t: now, v: 50020 },
      { id: 'bat1.current', t: now - 0.1, v: 12 },
      { id: 'bat1.current', t: now, v: 13 },
    ])
  })
  await page.waitForFunction(() => document.querySelectorAll('.scope-pane').length === 2)

  const signalList = page.getByRole('button', { name: 'Signal list', exact: true })
  await signalList.click()
  await page.locator('#motor-signal-list').waitFor({ state: 'hidden' })
  assert.equal(await page.locator('#motor-signal-list').isVisible(), false, 'signal list collapses on a wide window')
  await signalList.click()
  await page.locator('#motor-signal-list').waitFor({ state: 'visible' })
  assert.ok(
    await page.locator('#motor-signal-list', { hasText: 'Battery current' }).count(),
    'the battery-current signal is listed',
  )
  const resizer = page.getByRole('separator', { name: 'Resize signal list' })
  const listBoxBefore = await page.locator('#motor-signal-list').boundingBox()
  const resizerBox = await resizer.boundingBox()
  assert(listBoxBefore && resizerBox, 'the signal list and its resize handle are visible')
  await page.mouse.move(resizerBox.x + resizerBox.width / 2, resizerBox.y + 40)
  await page.mouse.down()
  await page.mouse.move(resizerBox.x + resizerBox.width / 2 + 120, resizerBox.y + 40, { steps: 6 })
  await page.mouse.up()
  const listBoxAfter = await page.locator('#motor-signal-list').boundingBox()
  assert(listBoxAfter && listBoxAfter.width >= listBoxBefore.width + 100, 'dragging widens the signal list')
  const resizedListWidth = Math.round(listBoxAfter.width)
  assert.equal(
    await page.evaluate(() => JSON.parse(localStorage.getItem('maggcs.actuator.scope')).listWidth),
    resizedListWidth,
    'the resized signal-list width is persisted',
  )
  const longSignalName = page.locator('.signal-row', { hasText: 'Command PWM M 1' }).locator('.signal-name')
  assert.equal(await longSignalName.getAttribute('title'), 'Command PWM M 1', 'the full signal name remains available as a tooltip')
  assert.equal(await longSignalName.evaluate((name) => name.scrollWidth <= name.clientWidth), true, 'widening the list shows the complete signal name')
  assert.equal(await page.evaluate(() => document.documentElement.classList.contains('scope-resizing')), false, 'releasing the pointer clears resize state')
  await resizer.focus()
  await page.keyboard.press('ArrowLeft')
  const listBoxKeyboard = await page.locator('#motor-signal-list').boundingBox()
  assert(listBoxKeyboard && Math.round(listBoxKeyboard.width) === resizedListWidth - 16, 'arrow keys resize the list by 16 px')
  await page.keyboard.press('End')
  const listBoxMax = await page.locator('#motor-signal-list').boundingBox()
  assert(listBoxMax && Math.round(listBoxMax.width) === 560, 'End uses the maximum practical list width')
  await page.keyboard.press('Home')
  const listBoxMin = await page.locator('#motor-signal-list').boundingBox()
  assert(listBoxMin && Math.round(listBoxMin.width) === 220, 'Home uses the minimum list width')
  await resizer.dblclick()
  const listBoxReset = await page.locator('#motor-signal-list').boundingBox()
  assert(listBoxReset && Math.round(listBoxReset.width) === 300, 'double-click restores the default list width')
  await resizer.focus()
  await page.keyboard.press('End')
  for (let i = 0; i < 5; i++) await page.keyboard.press('ArrowLeft')
  const persistedListWidth = Math.round((await page.locator('#motor-signal-list').boundingBox()).width)
  assert.equal(persistedListWidth, 480, 'keyboard resizing reaches a distinctive width')
  const signalSearch = page.getByRole('searchbox', { name: 'Search signals' })
  await signalSearch.fill('command pwm m 1')
  if ((await page.locator('.signal-row:visible').count()) === 0) {
    await page.locator('.signal-group-head', { hasText: 'Command output' }).click()
  }
  assert.equal(await page.locator('.signal-row:visible').count(), 1, 'signal search filters the compact list')
  assert.match(await page.locator('.signal-row:visible').textContent(), /Command PWM M 1/)
  await signalSearch.fill('')

  const plotCheckbox = (label, pane) => page.getByRole('checkbox', { name: `Plot ${label} in ${pane} plot`, exact: true })
  const selectionCommandsBefore = await page.evaluate(() => window.__bench.calls.filter((call) =>
    /^actuator_(?:start_|set_values|stop$|estop$|stream_set_rate$)/.test(call.command)).length)
  const topCanvasBeforeBottomSelection = await commandPane.locator('canvas').first().elementHandle()
  await bottomPane.getByRole('button', { name: 'Select signals for bottom plot', exact: true }).click()
  assert.equal(await page.getByRole('button', { name: 'Edit bottom plot signals', exact: true }).getAttribute('aria-pressed'), 'true')
  assert.equal(await plotCheckbox('Command PWM M 1', 'bottom').isChecked(), false, 'top output selection is not shared with the bottom plot')
  await plotCheckbox('Command PWM M 1', 'bottom').check()
  await bottomPane.locator('.u-label', { hasText: 'Command PWM M 1' }).waitFor()
  assert.equal(await commandPane.locator('.u-label', { hasText: 'Command PWM M 1' }).count(), 1, 'the same signal can appear in both plots')
  assert.equal(await topCanvasBeforeBottomSelection.evaluate((canvas) => canvas.isConnected), true, 'editing bottom selection leaves the top plot mounted')
  await plotCheckbox('Battery current 1', 'bottom').check()
  await bottomPane.locator('.u-label', { hasText: 'Battery current 1' }).waitFor()
  const bottomScales = await bottomPane.locator('.scope-pane-plot').evaluate((host) => Object.keys(host.__maggcsScopePlot.scales))
  assert(bottomScales.includes('µs') && bottomScales.includes('nT') && bottomScales.includes('A'), 'a plot freely combines output, magnetic and battery signals with separate unit axes')

  await plotCheckbox('Command PWM M 1', 'bottom').uncheck()
  assert.equal(await bottomPane.locator('.u-label', { hasText: 'Command PWM M 1' }).count(), 0)
  assert.equal(await commandPane.locator('.u-label', { hasText: 'Command PWM M 1' }).count(), 1, 'a bottom uncheck does not hide the top trace')
  const bottomCommandGroup = page.getByRole('checkbox', { name: 'Plot Command output (converted ACTUATOR_TEST) in bottom plot', exact: true })
  await bottomCommandGroup.check()
  assert.equal(await commandPane.locator('.u-label', { hasText: 'Command PWM M 4' }).count(), 0, 'group selection affects only the chosen plot')
  await bottomCommandGroup.uncheck()
  assert.equal(await commandPane.locator('.u-label', { hasText: 'Command PWM M 1' }).count(), 1, 'group hiding leaves the other plot unchanged')
  await plotCheckbox('Command PWM M 1', 'bottom').check()
  await plotCheckbox('Command PWM M 2', 'bottom').check()

  await signalList.click()
  await page.locator('#motor-signal-list').waitFor({ state: 'hidden' })
  await commandPane.getByRole('button', { name: 'Select signals for top plot', exact: true }).click()
  await page.locator('#motor-signal-list').waitFor({ state: 'visible' })
  assert.equal(await page.getByRole('button', { name: 'Edit top plot signals', exact: true }).getAttribute('aria-pressed'), 'true', 'the pane action opens its own signal selection')
  await plotCheckbox('Command PWM M 1', 'top').uncheck()
  assert.equal(await bottomPane.locator('.u-label', { hasText: 'Command PWM M 1' }).count(), 1, 'a top uncheck does not hide the bottom trace')
  await plotCheckbox('Command PWM M 1', 'top').check()
  await plotCheckbox('Command PWM M 4', 'top').check()
  await plotCheckbox('Command PWM M 4', 'top').uncheck()
  await page.evaluate(() => window.__bench.emit('actuator_samples', [
    { id: 'cmd.pwm4', t: Date.now() / 1000, v: 1500 },
  ]))
  await page.waitForFunction(() => [...document.querySelectorAll('.signal-row')].some((row) =>
    row.querySelector('.signal-name')?.textContent === 'Command PWM M 4'
      && row.querySelector('.signal-value')?.textContent === '1500 µs'))
  assert.equal(await plotCheckbox('Command PWM M 4', 'top').isChecked(), false, 'unchecking before the first sample is remembered by that plot')
  assert.equal(await page.evaluate(() => window.__bench.calls.filter((call) =>
    /^actuator_(?:start_|set_values|stop$|estop$|stream_set_rate$)/.test(call.command)).length), selectionCommandsBefore, 'signal selection emits no motor or stream commands')
  console.log('PASS: independent top/bottom selection, duplicate signals, mixed-unit axes, scoped group changes and pre-data unchecks')

  await page.evaluate(() => window.__bench.emit('actuator_samples', [
    { id: 'cmd.pwm2', t: Date.now() / 1000, v: 1500 },
  ]))
  await page.waitForFunction(() => [...document.querySelectorAll('.signal-row')].some((row) =>
    row.querySelector('.signal-name')?.textContent === 'Command PWM M 2' && row.querySelector('input')?.checked,
  ))
  await plotCheckbox('Command PWM M 2', 'top').uncheck()
  if ((await trackingFold.textContent()) === 'Fold') await trackingFold.click()

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
      const plots = await page.locator('.scope-pane').evaluateAll((panes) => panes.map((pane) => pane.getBoundingClientRect().height))
      assert.equal(plots.length, 2, 'two independent plots stay present')
      assert(plots.every((height) => height >= 200), 'each stacked plot keeps at least 200 px height')
    } else {
      assert(layout.startTop < layout.midTop, 'stacked start controls precede the plotting workspace')
    }
    const geometryDrift = await page.locator('.scope-panes').evaluate(async (root) => {
      const samples = []
      for (let frame = 0; frame < 12; frame++) {
        await new Promise(requestAnimationFrame)
        samples.push([...root.querySelectorAll('.scope-pane')].map((pane) => {
          const rect = pane.getBoundingClientRect()
          return [rect.y, rect.width, rect.height]
        }).flat())
      }
      return Math.max(...samples[0].map((_, index) =>
        Math.max(...samples.map((sample) => sample[index])) - Math.min(...samples.map((sample) => sample[index]))))
    })
    assert(geometryDrift < 1, 'redraws do not cause a plot-size feedback loop')
    console.log(`PASS: ${width}x${height} bounded layout, reachable plots/capture, fixed stop controls`)
    if (shots) {
      await page.locator('#motor-signal-list').evaluate((list) => { list.scrollTop = 0 })
      await page.locator('.scope-grid').scrollIntoViewIfNeeded()
      await page.screenshot({ path: join(shots, `motor-controls-${width}x${height}.png`) })
      if (width === 1366) await page.screenshot({ path: join(shots, 'motor-controls-independent-plots-1366x768.png') })
    }
  }
  await page.setViewportSize({ width: 1366, height: 768 })
  if ((await trackingFold.textContent()) === 'Unfold') await trackingFold.click()

  // DShot must rebuild the existing plot and cannot relabel its PWM history.
  const previousPwmOutputs = await page.evaluate(() => structuredClone(window.__bench.scopeOutputs()))
  await commandPane.locator('canvas').first().waitFor({ state: 'attached' })
  const pwmCanvas = await commandPane.locator('canvas').first().elementHandle()
  await page.evaluate(() => {
    const s = window.__bench.status
    const dshot = { kind: 'dshot', rate: 'r600' }
    s.output_protocols = { main: dshot, aux: dshot }
    s.pwm_models = s.pwm_models.map((model) => ({
      ...model,
      protocol: dshot,
      min_us: 110,
      max_us: 1999,
      dshot_min: 0.055,
    }))
    s.dshot_bidir_en = 1
    s.fc_output_pwm = [550, 600, 580, null]
    s.esc_rpm = [
      { esc: 1, rpm: 5000, age_s: 0.1, stale: false },
      { esc: 2, rpm: null, age_s: null, stale: false },
    ]
  })
  await page.waitForFunction(() =>
    document.querySelector('.motor-table')?.textContent?.includes('DShot600'),
  )
  assert.match(await page.getByTestId('motor-active-protocol').textContent(), /DShot600/)
  const dshotRow = await page.locator('.motor-table tbody tr').first().textContent()
  assert.match(dshotRow, /thr/, 'DShot tracking uses throttle units')
  assert.doesNotMatch(dshotRow, /µs/, 'DShot tracking never uses microseconds')
  await page.waitForFunction((canvas) => !canvas.isConnected, pwmCanvas)
  assert.equal(await pwmCanvas.evaluate((canvas) => canvas.isConnected), false, 'a protocol/unit change rebuilds the actual mounted plot')
  await emitOutputCurves({ 'cmd.pwm1': 550, 'fc.motor1': 548, 'cmd.pwm2': 600 })
  await page.waitForFunction(() =>
    [...document.querySelectorAll('.signal-name')].some(
      (el) => el.textContent === 'Command throttle M 1',
    ),
  )
  const throttleTrace = page.locator('.signal-row', { hasText: 'Command throttle M 1' })
  await page.waitForFunction(() => [...document.querySelectorAll('.signal-row')].some((row) =>
    row.querySelector('.signal-name')?.textContent === 'Command throttle M 1' && row.querySelector('.signal-value')?.textContent === '550 thr',
  ))
  assert.equal(await commandPane.locator('.u-label', { hasText: 'Command throttle M 1' }).count(), 1)
  assert.equal(await commandPane.locator('.u-label', { hasText: 'Command PWM M 1' }).count(), 0)
  assert.equal(await page.getByLabel('Color for Command throttle M 1', { exact: true }).inputValue(), '#ff00aa', 'protocol changes retain the signal color')
  assert.equal(await plotCheckbox('Command throttle M 2', 'top').isChecked(), false, 'unit changes preserve an unchecked signal')
  await page.evaluate((old) => window.__bench.emit('actuator_samples', [
    { id: 'cmd.pwm1', t: Date.now() / 1000, v: 1700, output: old[0] },
    { id: 'cmd.pwm1', t: Date.now() / 1000, v: 900, output: null },
  ]), previousPwmOutputs)
  await page.waitForTimeout(600)
  assert.equal(await throttleTrace.locator('.signal-value').textContent(), '550 thr', 'late or untagged samples cannot enter the current unit history')
  await page.locator('.rpm-card').scrollIntoViewIfNeeded()
  const rpmText = await page.locator('.rpm-card').textContent()
  assert.match(rpmText, /ESC1/, 'the RPM card lists the reporting ESC')
  assert.match(rpmText, /5000/, 'live RPM is shown')
  // RPM hold, calibration, mapping verification and speed metrics are removed.
  assert.equal(await page.locator('.rpm-metric').count(), 0, 'no speed metrics')
  assert.equal(await page.locator('.rpm-hold-row').count(), 0, 'no RPM hold controls')
  assert.equal(await page.locator('.rpm-map').count(), 0, 'no motor to ESC mapping')
  assert.equal(await page.locator('.rpm-verify').count(), 0, 'no verification pulse')
  const rpmCardText = await page.locator('.rpm-card').textContent()
  assert.doesNotMatch(rpmCardText, /RPM hold|Motor → ESC mapping|Verify M/)
  assert.equal(await page.locator('.rpm-card button,.rpm-card input,.rpm-card select').count(), 0, 'RPM telemetry has no control actions')
  if (shots) {
    await page.setViewportSize({ width: 1366, height: 768 })
    await page.locator('.rpm-card').scrollIntoViewIfNeeded()
    await waitForOutputPlot()
    await page.screenshot({ path: join(shots, 'motor-controls-dshot-rpm-1366x768.png') })
  }

  // MR-21: independently saved choices never preview or start motors. The
  // pending fixture separates cancellation of START from a late tone reply.
  const startTone = page.getByLabel('Start tone', { exact: true })
  const stopTone = page.getByLabel('Stop tone', { exact: true })
  assert.equal(await startTone.inputValue(), 'rising')
  assert.equal(await stopTone.inputValue(), 'falling')
  await resetCalls()
  for (const value of ['off', 'sweep', 'triple', 'falling', 'rising']) {
    await startTone.selectOption(value)
    await page.waitForFunction((value) => JSON.parse(localStorage.getItem('maggcs.actuator.tones'))?.start === value, value)
    assert.equal(await stopTone.inputValue(), 'falling', 'changing start tone preserves stop choice')
  }
  await startTone.selectOption('sweep')
  for (const value of ['off', 'rising', 'sweep', 'triple']) {
    await stopTone.selectOption(value)
    await page.waitForFunction((value) => JSON.parse(localStorage.getItem('maggcs.actuator.tones'))?.stop === value, value)
    assert.equal(await startTone.inputValue(), 'sweep', 'changing stop tone preserves start choice')
  }
  assert.equal(await calls('actuator_set_tones'), 10, 'one settings request per selection')
  assert.equal(await calls('actuator_start_manual'), 0, 'settings never start motors')
  assert.equal(await calls('actuator_stop'), 0, 'settings never stop motors')
  assert.equal(await calls('console_send'), 0, 'UI cannot inject arbitrary shell commands for a tone')
  if (shots) await page.screenshot({ path: join(shots, 'motor-controls-am32-tones-1366x768.png') })
  for (const action of ['header', 'sidebar', 'escape', 'off']) {
    await ensureUnlocked()
    await page.evaluate(() => { window.__bench.holdMotorStart = true })
    await resetCalls()
    await holdStart()
    await page.waitForFunction(() => document.querySelector('[data-testid="motor-session-state"]')?.textContent === 'STARTING')
    assert.equal(await start().textContent(), 'CANCEL START')
    assert.equal(await start().isEnabled(), true)
    assert.equal(await startTone.isDisabled(), true)
    assert.equal(await stopTone.isDisabled(), true)
    assert.equal(await page.getByRole('button', { name: 'Preset', exact: true }).isDisabled(), true, 'source is frozen during the start tone')
    assert.equal(await page.locator('.motor-control-row input[type="range"]').first().isDisabled(), true, 'pending start uses the confirmed slider values')
    assert.equal(await page.getByRole('checkbox', { name: 'Synchronize all', exact: true }).isDisabled(), true)
    assert.equal(await page.locator('.actuator-header .stop-tab').isEnabled(), true, 'header STOP cancels a tone-stage start')
    if (shots && action === 'header') {
      await page.waitForFunction(() => document.querySelector('[data-testid="motor-tone-status"]')?.textContent?.includes('playing'))
      await page.screenshot({ path: join(shots, 'motor-controls-am32-starting-1366x768.png') })
    }
    if (action === 'escape') await page.keyboard.press('Escape')
    else if (action === 'off') await motorControl.click()
    else await (action === 'header' ? page.locator('.actuator-header .stop-tab') : start()).click()
    await page.waitForFunction(() => document.querySelector('[data-testid="motor-session-state"]')?.textContent !== 'STARTING')
    assert.equal(await calls(action === 'escape' ? 'actuator_estop' : action === 'off' ? 'actuator_set_motor_control' : 'actuator_stop'), 1)
    assert.equal(await page.evaluate(() => window.__bench.pendingMotorStart?.cancelled), true)
    assert.equal(await page.evaluate(() => window.__bench.status.session), action === 'escape' ? 'emergency' : 'idle')
    await page.evaluate(() => window.__bench.completeMotorStart())
    assert.equal(await page.evaluate(() => window.__bench.status.output_values.length), 0, 'late tone completion cannot start output')
    if (action === 'escape') {
      assert.equal(await page.getByTestId('motor-session-state').textContent(), 'EMERGENCY')
      await page.getByRole('button', { name: 'RESET', exact: true }).click()
    }
    await page.waitForFunction(() => !document.querySelector('select[aria-label="Start tone"]')?.disabled)
  }
  console.log('PASS: independent AM32 tone choices persist; header/sidebar/Escape cancel pending starts without late output')

  // MR-18b: exact selection, unknown-support confirmation, immediate edits,
  // receiver state and reconnect restoration stay scoped to the dedicated link.
  await page.getByRole('button', { name: 'Streams…', exact: true }).click()
  const streamPanel = page.getByRole('region', { name: 'MAVLink stream manager' })
  const streamSearch = page.getByRole('textbox', { name: 'Search standard MAVLink messages' })
  await page.getByTestId('stream-row-147').waitFor()
  const beforeMapping = await calls('actuator_stream_set_rate')
  for (const [alias, name, id] of [['sensor_gps', 'GPS_RAW_INT', 24], ['sensor_mag', 'SCALED_IMU', 26], ['actuator_outputs', 'SERVO_OUTPUT_RAW', 36], ['vehicle_magnetometer', 'HIGHRES_IMU', 105], ['battery_status', 'BATTERY_STATUS', 147], ['esc_status', 'ESC_STATUS', 291]]) {
    await streamSearch.fill(alias)
    await streamPanel.getByRole('button', { name: `${name} ${id}`, exact: true }).click()
    assert.match(await page.getByTestId('stream-uorb-mapping').innerText(), new RegExp(alias))
    assert.equal(await page.getByTestId('stream-uorb-mapping').getByRole('link').getAttribute('href'), `https://github.com/PX4/PX4-Autopilot/blob/v1.17.0/src/modules/mavlink/streams/${name}.hpp`)
  }
  await streamSearch.fill('vehicle_imu')
  await streamPanel.getByRole('status').filter({ hasText: 'Shared uORB source' }).waitFor()
  assert.equal(await streamPanel.locator('.stream-result').count(), 2)
  assert.ok(await streamPanel.getByRole('button', { name: 'Add stream', exact: true }).isDisabled())
  if (shots) {
    await waitForStreamViewport()
    await page.screenshot({ path: join(shots, 'motor-stream-uorb-ambiguous-1366x768.png') })
  }
  await streamPanel.getByRole('button', { name: 'SCALED_IMU 26', exact: true }).click()
  assert.match(await page.getByTestId('stream-uorb-mapping').innerText(), /Instance 0/)
  if (shots) await page.screenshot({ path: join(shots, 'motor-stream-uorb-mapping-1366x768.png') })
  await streamSearch.fill('unmapped_custom_uorb_topic')
  await streamPanel.getByText(/installed firmware cannot expose arbitrary uORB/).waitFor()
  assert.ok(await streamPanel.getByRole('button', { name: 'Add stream', exact: true }).isDisabled())
  assert.equal(await calls('actuator_stream_set_rate'), beforeMapping, 'source inspection, ambiguity and unmapped topics send no request')
  console.log('PASS: six verified uORB aliases expose sources/evidence; shared and unmapped queries never send a request')
  for (const [query, name, id] of [['BATTERY_STATUS', 'BATTERY_STATUS', 147], ['26', 'SCALED_IMU', 26], ['HIGHRES_IMU', 'HIGHRES_IMU', 105]]) {
    await streamSearch.fill(query)
    await page.waitForFunction(({ name, id }) => document.querySelector('.stream-result')?.textContent === `${name}${id}`, { name, id })
    assert.equal(await streamPanel.getByRole('button', { name: 'Add stream', exact: true }).isDisabled(), true, 'search never implicitly selects a message')
  }
  await streamSearch.fill('147')
  await streamPanel.getByRole('button', { name: 'BATTERY_STATUS 147', exact: true }).click()
  await streamPanel.getByText('Fields and wire units (2)', { exact: true }).click()
  assert.match(await streamPanel.locator('.stream-fields').innerText(), /voltages uint16_t\[10\] mV/)
  await streamSearch.fill('current_battery')
  await page.waitForFunction(() => document.querySelectorAll('.stream-result').length === 2)
  assert.equal(await streamPanel.getByRole('button', { name: 'Add stream', exact: true }).isDisabled(), true, 'ambiguous field search sends nothing')
  const beforeStreams = await calls('actuator_stream_set_rate')
  await streamSearch.fill('SCALED_IMU')
  await streamPanel.getByRole('button', { name: 'SCALED_IMU 26', exact: true }).click()
  const newRate = streamPanel.getByRole('spinbutton', { name: 'New stream rate (Hz)' })
  await newRate.fill('0')
  assert.equal(await streamPanel.getByRole('button', { name: 'Add stream', exact: true }).isDisabled(), true)
  await newRate.fill('10')
  await streamPanel.getByRole('button', { name: 'Add stream', exact: true }).click()
  await page.getByTestId('stream-row-26').waitFor()
  assert.equal(await calls('actuator_stream_set_rate'), beforeStreams + 1, 'one exact add request')
  assert.equal(await page.getByTestId('stream-hz-26').textContent(), '—', 'requested rate is not displayed as observed')
  const beforeEdit = await calls('actuator_stream_set_rate')
  await streamPanel.getByRole('spinbutton', { name: 'Rate for BATTERY_STATUS' }).fill('20')
  await page.waitForFunction(() => window.__bench.streams.statuses.find((row) => row.message_id === 147)?.requested_rate.hz === 20)
  assert.equal(await calls('actuator_stream_set_rate'), beforeEdit + 1, 'valid rate edit applies immediately once')
  await streamPanel.getByRole('spinbutton', { name: 'Rate for BATTERY_STATUS' }).fill('0')
  assert.equal(await calls('actuator_stream_set_rate'), beforeEdit + 1, 'invalid rate edit sends nothing')
  await streamPanel.getByRole('spinbutton', { name: 'Rate for BATTERY_STATUS' }).fill('20')
  // Reverting a draft while the previous write is unresolved must enqueue the
  // latest value even when it equals the last acknowledged backend value.
  const beforeRevert = await calls('actuator_stream_set_rate')
  await page.evaluate(() => { window.__bench.holdStreamWrite = true })
  await streamPanel.getByRole('spinbutton', { name: 'Rate for BATTERY_STATUS' }).fill('25')
  await page.waitForFunction(() => window.__bench.pendingStreamWrite !== null)
  await streamPanel.getByRole('spinbutton', { name: 'Rate for BATTERY_STATUS' }).fill('20')
  await page.evaluate(() => window.__bench.completeStreamWrite())
  await page.waitForFunction((count) => window.__bench.calls.filter((call) => call.command === 'actuator_stream_set_rate').length === count
    && window.__bench.streams.statuses.find((row) => row.message_id === 147)?.requested_rate.hz === 20, beforeRevert + 2)
  await page.evaluate(() => Object.assign(window.__bench.streams.statuses.find((row) => row.message_id === 26), { observed_hz: 9.8, last_seen_age_ms: 50, receive_state: 'receiving' }))
  await page.waitForFunction(() => document.querySelector('[data-testid="stream-hz-26"]')?.textContent === '9.8 Hz')
  await waitForStreamViewport()
  if (shots) await page.screenshot({ path: join(shots, 'motor-stream-manager-live-1366x768.png') })
  await page.evaluate(() => Object.assign(window.__bench.streams.statuses.find((row) => row.message_id === 26), { observed_hz: null, last_seen_age_ms: 5000, receive_state: 'stale' }))
  await page.waitForFunction(() => document.querySelector('[data-testid="stream-state-26"]')?.textContent === 'stale')
  const beforeDisable = await calls('actuator_stream_set_rate')
  await streamPanel.getByRole('button', { name: 'Disable SCALED_IMU' }).click()
  await page.waitForFunction(() => window.__bench.streams.statuses.find((row) => row.message_id === 26)?.requested_rate.kind === 'disabled')
  assert.equal(await calls('actuator_stream_set_rate'), beforeDisable + 1)
  await page.evaluate(() => Object.assign(window.__bench.streams.statuses.find((row) => row.message_id === 26), { receive_state: 'receiving', observed_hz: 10 }))
  await page.waitForFunction(() => document.querySelector('[data-testid="stream-state-26"]')?.textContent.includes('still receiving'))
  await streamSearch.fill('MISSION_REQUEST_LIST')
  await streamPanel.getByRole('button', { name: 'MISSION_REQUEST_LIST 43', exact: true }).click()
  assert.equal(await streamPanel.getByRole('button', { name: 'Add stream', exact: true }).isDisabled(), true, 'unknown support requires an explicit choice')
  await streamPanel.getByRole('checkbox', { name: /PX4 support unknown/ }).check()
  await streamPanel.getByRole('button', { name: 'Add stream', exact: true }).click()
  await page.getByTestId('stream-row-43').waitFor()
  await streamSearch.fill('unmapped_custom_uorb_topic')
  await streamPanel.getByText(/No standard message matches/).waitFor()
  assert.equal(await streamPanel.getByRole('button', { name: 'Add stream', exact: true }).isDisabled(), true)
  const beforeReconnect = await calls('actuator_stream_set_rate')
  await page.evaluate(() => {
    window.__bench.streams.transport_generation += 1
    for (const row of window.__bench.streams.statuses) Object.assign(row, { observed_hz: null, last_seen_age_ms: null, receive_state: 'waiting' })
  })
  await page.getByTestId('stream-target').filter({ hasText: 'generation 2' }).waitFor()
  assert.equal(await calls('actuator_stream_set_rate'), beforeReconnect, 'UI does not duplicate backend reconnect restoration')
  assert.equal(await streamPanel.getByRole('spinbutton', { name: 'Rate for BATTERY_STATUS' }).inputValue(), '20')
  await ensureUnlocked()
  await holdStart()
  await page.waitForFunction(() => document.querySelector('[data-testid="motor-session-state"]')?.textContent === 'RUNNING')
  await page.evaluate(() => { window.__bench.holdStreamWrite = true })
  await streamPanel.getByRole('spinbutton', { name: 'Rate for BATTERY_STATUS' }).fill('30')
  await page.waitForFunction(() => window.__bench.pendingStreamWrite !== null)
  await page.locator('.actuator-header .stop-tab').click()
  await page.waitForFunction(() => window.__bench.status.session === 'idle')
  assert.equal(await page.evaluate(() => window.__bench.pendingStreamWrite !== null), true, 'STOP completes while stream request waits')
  await page.evaluate(() => window.__bench.completeStreamWrite())
  await ensureUnlocked()
  await holdStart()
  await page.waitForFunction(() => window.__bench.status.session === 'running')
  await page.evaluate(() => { window.__bench.holdStreamWrite = true })
  await streamPanel.getByRole('spinbutton', { name: 'Rate for BATTERY_STATUS' }).fill('35')
  await page.waitForFunction(() => window.__bench.pendingStreamWrite !== null)
  await page.keyboard.press('Escape')
  await page.waitForFunction(() => document.querySelector('[data-testid="motor-session-state"]')?.textContent === 'EMERGENCY')
  assert.equal(await page.evaluate(() => window.__bench.pendingStreamWrite !== null), true, 'Escape stops motors while the stream editor has focus')
  await page.evaluate(() => window.__bench.completeStreamWrite())
  assert.equal(await page.getByTestId('motor-session-state').textContent(), 'EMERGENCY', 'late stream reply preserves the latch')
  await page.getByRole('button', { name: 'RESET', exact: true }).click()
  await streamSearch.fill('BATTERY_STATUS')
  await streamPanel.getByRole('button', { name: 'BATTERY_STATUS 147', exact: true }).click()
  await streamPanel.getByText('Fields and wire units (2)', { exact: true }).click()
  await page.evaluate(() => Object.assign(window.__bench.streams.statuses.find((row) => row.message_id === 147), { observed_hz: 34.6, last_seen_age_ms: 30, receive_state: 'receiving' }))
  await page.waitForFunction(() => document.querySelector('[data-testid="stream-hz-147"]')?.textContent === '34.6 Hz')
  for (const viewport of [{ width: 1024, height: 640 }, { width: 1366, height: 768 }]) {
    await page.setViewportSize(viewport)
    await streamPanel.scrollIntoViewIfNeeded()
    await streamPanel.locator('.stream-request-list').evaluate((list) => { list.scrollTop = 0 })
    await waitForStreamViewport()
    assert.equal(await page.locator('.actuator-header .estop-btn').isVisible(), true)
    const overflow = await streamPanel.evaluate((panel) => panel.scrollWidth > panel.clientWidth + 1)
    assert.equal(overflow, false, 'stream panel has no horizontal overflow')
    if (shots) await page.screenshot({ path: join(shots, `motor-stream-manager-${viewport.width}x${viewport.height}.png`) })
  }
  await page.evaluate(() => document.documentElement.classList.add('field-mode'))
  await waitForStreamViewport()
  const smallStreamControls = await streamPanel.evaluate((panel) => [...panel.querySelectorAll('button,input:not([type="checkbox"]),summary')]
    .filter((element) => element.getClientRects().length > 0)
    .filter((element) => element.getBoundingClientRect().width < 44 || element.getBoundingClientRect().height < 44)
    .map((element) => element.getAttribute('aria-label') ?? element.textContent))
  assert.deepEqual(smallStreamControls, [], 'stream controls retain Field-mode touch sizes')
  if (shots) await page.screenshot({ path: join(shots, 'motor-stream-manager-field-1366x768.png') })
  await page.evaluate(() => document.documentElement.classList.remove('field-mode'))
  await streamPanel.getByRole('button', { name: 'Close streams' }).click()
  assert.equal(await page.locator('.scope-pane').count(), 2, 'closing streams restores both existing plots')
  console.log('PASS: stream selection/units, immediate rate edits, disable, observed/stale state, scoped persistence and stop priority')

  await page.evaluate(() => { window.__bench.status.dshot_3d_enable = 1 })
  await page.waitForFunction(() =>
    document.body.textContent?.includes('DSHOT_3D_ENABLE=1 blocks this output model'),
  )
  await page.evaluate(() => { window.__bench.status.dshot_3d_enable = 0 })

  // MAIN1 and AUX1 belong to different motor traces and different unit axes.
  const dshotFixture = await page.evaluate(() => ({
    map: structuredClone(window.__bench.status.output_map),
    models: structuredClone(window.__bench.status.pwm_models),
  }))
  await page.evaluate(() => {
    const s = window.__bench.status
    const pwm = { kind: 'pwm', rate_hz: 400 }
    const dshot = { kind: 'dshot', rate: 'r600' }
    s.output_protocols = { main: pwm, aux: dshot }
    s.output_map.motors[1].channel = { port: 'aux', channel: 1 }
    s.output_map.motors[3].channel = { port: 'aux', channel: 2 }
    s.pwm_models = s.pwm_models.map((model, index) => ({
      ...model,
      protocol: index % 2 === 0 ? pwm : dshot,
      min_us: index % 2 === 0 ? 1100 : 110,
      max_us: index % 2 === 0 ? 1900 : 1999,
    }))
    s.fc_output_pwm = [1500, 750, 1500, 750]
  })
  await page.waitForFunction(() => document.querySelector('.motor-table tbody tr')?.textContent?.includes('PWM 400 Hz'))
  await emitOutputCurves({ 'cmd.pwm1': 1500, 'fc.motor1': 1500, 'fc.motor2': 750 })
  await page.waitForFunction(() => [...document.querySelectorAll('.signal-row')].some((row) =>
    row.querySelector('.signal-name')?.textContent === 'FC output M 2 (AUX1)' && row.querySelector('.signal-value')?.textContent === '750 thr',
  ))
  assert.equal(await page.locator('.signal-row', { hasText: 'FC output M 1 (MAIN1)' }).locator('.signal-value').textContent(), '1500 µs')
  assert.equal(await commandPane.locator('.u-label', { hasText: 'FC output M 1 (MAIN1)' }).count(), 1)
  assert.equal(await commandPane.locator('.u-label', { hasText: 'FC output M 2 (AUX1)' }).count(), 1)
  if (shots) {
    await page.setViewportSize({ width: 1366, height: 768 })
    await commandPane.scrollIntoViewIfNeeded()
    await waitForOutputPlot()
    await page.screenshot({ path: join(shots, 'motor-controls-mixed-output-units-1366x768.png') })
  }
  await page.evaluate((fixture) => {
    const s = window.__bench.status
    s.output_map = fixture.map
    s.pwm_models = fixture.models
    const dshot = { kind: 'dshot', rate: 'r600' }
    s.output_protocols = { main: dshot, aux: dshot }
  }, dshotFixture)
  await page.waitForFunction(() => document.querySelector('.motor-table tbody tr')?.textContent?.includes('DShot600'))

  // One active motor, then per-ESC missing/stale data and a live second bank.
  const fullMap = await page.evaluate(() => structuredClone(window.__bench.status.output_map))
  await page.evaluate(() => {
    window.__bench.status.output_map.motors = window.__bench.status.output_map.motors.slice(0, 1)
  })
  await page.waitForFunction(() => document.querySelectorAll('.motor-table tbody > tr:not(.motor-detail)').length === 1)
  await page.evaluate(() => {
    const s = window.__bench.status
    s.esc_rpm = [
      { esc: 1, rpm: null, age_s: null, stale: false },
      { esc: 2, rpm: 3000, age_s: 1.2, stale: true },
      { esc: 3, rpm: null, age_s: null, stale: false },
      { esc: 4, rpm: null, age_s: null, stale: false },
      { esc: 5, rpm: 5000, age_s: 0.1, stale: false },
      { esc: 6, rpm: 6000, age_s: 0.1, stale: false },
      { esc: 7, rpm: 0, age_s: 0.1, stale: false },
      { esc: 8, rpm: 8000, age_s: 0.1, stale: false },
    ]
    const t = Date.now() / 1000
    window.__bench.emit('actuator_samples', [
      { id: 'esc5.rpm', t, v: 5000 },
      { id: 'esc6.rpm', t, v: 6000 },
      { id: 'esc7.rpm', t, v: 0 },
      { id: 'esc8.rpm', t, v: 8000 },
    ])
  })
  await page.waitForTimeout(1200)
  await page.waitForFunction(() => {
    const text = document.querySelector('.rpm-card')?.textContent
    return text?.includes('ESC1 —') && text.includes('ESC2 3000 stale')
  })
  const missingChip = page.locator('.rpm-chip').first()
  assert.equal(
    await missingChip.getAttribute('title'),
    'this ESC has not reported',
    'a missing ESC is not rendered as zero',
  )
  assert.equal(await page.locator('.rpm-chip').count(), 8, 'all eight ESC indices are shown')
  assert.equal(await page.locator('.rpm-chip.is-live').count(), 4, 'a live second bank does not refresh the first')
  assert.equal(await page.locator('.rpm-chip.is-stale').count(), 1, 'stale data keeps its own state')
  assert.equal(await page.locator('.rpm-chip.is-missing').count(), 3, 'unseen ESCs remain missing')
  assert.equal(await page.locator('.rpm-chip.is-live', { hasText: 'ESC7' }).textContent(), 'ESC7 0', 'reported zero is live data')
  for (const esc of [5, 6, 7, 8]) {
    await page.locator('.signal-row', { hasText: `ESC rpm ${esc}` }).waitFor()
  }
  const zeroRpm = page.locator('.signal-row', { hasText: 'ESC rpm 7' })
  assert.equal(await zeroRpm.locator('.signal-value').textContent(), '0 rpm', 'second-bank scope data is not discarded')
  if (shots) {
    await page.setViewportSize({ width: 1366, height: 768 })
    await page.locator('.rpm-card').scrollIntoViewIfNeeded()
    await page.screenshot({ path: join(shots, 'motor-controls-esc-banks-1366x768.png') })
  }

  // Unknown protocols visibly block START instead of falling back to PWM.
  await page.evaluate(() => {
    const s = window.__bench.status
    const unknown = { kind: 'unknown' }
    s.output_protocols = { main: unknown, aux: unknown }
    s.pwm_models = s.pwm_models.map((model) => ({ ...model, protocol: unknown }))
    s.dshot_bidir_en = 0
    s.output_map = structuredClone(window.__bench.status.output_map)
    s.output_map.motors = window.__bench.status.output_map.motors.slice(0, 4)
  })
  await page.evaluate((map) => { window.__bench.status.output_map = map }, fullMap)
  await page.waitForFunction(() =>
    document.querySelector('.protocol-pair')?.textContent?.includes('unknown'),
  )
  await page.waitForFunction(() => document.querySelector('.start-btn')?.disabled === true)
  await page.waitForFunction(() =>
    [...document.querySelectorAll('.fold-head')]
      .find((el) => el.textContent?.includes('Connection & output details'))
      ?.getAttribute('aria-expanded') === 'true',
  )
  assert.equal(
    await outputFold.locator('.fold-state').textContent(),
    'required',
    'an output warning forces its details open',
  )
  assert.equal(await trackingFold.textContent(), 'required')
  assert.equal(await trackingFold.isDisabled(), true)
  assert.equal(await page.locator('.motor-detail').count(), 1, 'pinned detail survives a protocol warning')
  const unknownRow = await page.locator('.motor-table tbody tr').first().textContent()
  assert.match(unknownRow, /Unit unknown/)
  assert.doesNotMatch(unknownRow, /µs|thr/, 'unknown protocols do not inherit guessed output units')

  await page.evaluate(() => {
    const s = window.__bench.status
    const pwm = { kind: 'pwm', rate_hz: 400 }
    s.output_protocols = { main: pwm, aux: pwm }
    s.pwm_models = s.pwm_models.map((model) => ({
      ...model,
      protocol: pwm,
      min_us: 1100,
      max_us: 1900,
      dshot_min: 0,
    }))
    s.dshot_bidir_en = 0
    s.fc_output_pwm = [1500, 1503, 1740, null]
  })
  await page.waitForFunction(() =>
    document.querySelector('.motor-table')?.textContent?.includes('PWM 400 Hz'),
  )

  await emitOutputCurves({ 'cmd.pwm1': 1500, 'fc.motor1': 1498 })
  await page.waitForFunction(() => [...document.querySelectorAll('.signal-row')].some((row) =>
    row.querySelector('.signal-name')?.textContent === 'Command PWM M 1' && row.querySelector('.signal-value')?.textContent === '1500 µs',
  ))
  const pulseCanvas = await commandPane.locator('canvas').first().elementHandle()
  await page.evaluate(() => {
    const s = window.__bench.status
    const oneshot = { kind: 'oneshot' }
    s.output_protocols = { main: oneshot, aux: oneshot }
    s.pwm_models = s.pwm_models.map((model) => ({ ...model, protocol: oneshot }))
    s.fc_output_pwm = [1200, 1200, 1200, null]
  })
  await page.waitForFunction(() => document.querySelector('.motor-table tbody tr')?.textContent?.includes('OneShot'))
  assert.match(await page.getByTestId('motor-active-protocol').textContent(), /OneShot/)
  await page.waitForFunction((canvas) => !canvas.isConnected, pulseCanvas)
  assert.equal(await pulseCanvas.evaluate((canvas) => canvas.isConnected), false, 'a new configuration rebuilds the plot even when pulse units stay µs')
  await emitOutputCurves({ 'cmd.pwm1': 1200, 'fc.motor1': 1200 })
  await page.waitForFunction(() => [...document.querySelectorAll('.signal-row')].some((row) =>
    row.querySelector('.signal-name')?.textContent === 'Command PWM M 1' && row.querySelector('.signal-value')?.textContent === '1200 µs',
  ))
  assert.equal(await commandPane.locator('.u-label', { hasText: 'Command PWM M 1' }).count(), 1)
  assert.equal(await commandPane.locator('.u-label', { hasText: 'Command throttle M 1' }).count(), 0)
  if (shots) {
    await page.setViewportSize({ width: 1366, height: 768 })
    await commandPane.scrollIntoViewIfNeeded()
    await waitForOutputPlot()
    await page.screenshot({ path: join(shots, 'motor-controls-oneshot-1366x768.png') })
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
  assert.equal(await page.getByRole('switch', { name: 'Motor control', exact: true }).getAttribute('aria-checked'), 'false', 'ON permission is never persisted across window reload')
  assert.equal(await start().isDisabled(), true)
  await page.getByRole('button', { name: 'Streams…', exact: true }).click()
  await page.getByTestId('stream-row-147').waitFor()
  assert.equal(await page.getByRole('spinbutton', { name: 'Rate for BATTERY_STATUS' }).inputValue(), '35', 'backend preferences survive window reload')
  assert.equal(await calls('actuator_stream_set_rate'), 0, 'window reload sends no duplicate restore writes')
  await page.getByRole('button', { name: 'Close streams' }).click()
  assert.equal(await page.getByLabel('Start tone', { exact: true }).inputValue(), 'sweep', 'start tone survives reload')
  assert.equal(await page.getByLabel('Stop tone', { exact: true }).inputValue(), 'triple', 'stop tone survives reload')
  assert.equal(await page.evaluate(() => document.documentElement.classList.contains('field-mode')), true, 'field mode is restored at window boot')
  assert.equal(await page.evaluate(() => document.documentElement.dataset.theme), 'light', 'theme is restored at window boot')
  assert.equal(
    await page.evaluate(() => JSON.parse(localStorage.getItem('maggcs.actuator.scope')).colors['cmd.pwm1']),
    '#ff00aa',
    'trace color survives reload',
  )
  if (!(await page.locator('#motor-signal-list').isVisible())) {
    await page.getByRole('button', { name: 'Signal list', exact: true }).click()
  }
  assert.equal(
    Math.round((await page.locator('#motor-signal-list').boundingBox()).width),
    persistedListWidth,
    'the resized signal-list width survives reload',
  )
  assert.equal(await plotCheckbox('Command PWM M 2', 'top').isChecked(), false, 'reload preserves the top-only uncheck')
  await page.getByRole('button', { name: 'Edit bottom plot signals', exact: true }).click()
  assert.equal(await plotCheckbox('Command PWM M 2', 'bottom').isChecked(), true, 'reload preserves the independent bottom selection of the same signal')
  assert.equal(await plotCheckbox('Battery current 1', 'bottom').isChecked(), true, 'mixed-category selection survives reload')
  await signalSearch.fill('FC output M 1')
  assert.equal(await plotCheckbox('FC output M 1 (MAIN1)', 'bottom').isChecked(), false, 'reload does not copy top feedback into the bottom plot')
  await signalSearch.fill('')
  await page.getByRole('button', { name: 'Edit top plot signals', exact: true }).click()
  await page.getByRole('button', { name: 'Clear', exact: true }).click()
  await page.evaluate(() => window.__bench.emit('actuator_samples', [
    { id: 'cmd.pwm2', t: Date.now() / 1000, v: 1510 },
  ]))
  await page.waitForTimeout(600)
  assert.equal(await plotCheckbox('Command PWM M 2', 'top').isChecked(), false, 'Clear and new samples do not revive the top-only uncheck')
  assert.equal(await bottomPane.locator('.u-label', { hasText: 'Command PWM M 2' }).count(), 1)
  console.log('PASS: resizable list and separate plot selections survive reload, Clear and new samples')

  const legacySeen = ['mag.raw_total', 'mag.cal_total', 'bat1.current', ...Array.from({ length: 4 }, (_, motor) => [
    `sitl.servo${motor + 1}`, `fwd.cmd${motor + 1}`, `fc.motor${motor + 1}`,
    `esc${motor + 1}.rpm`, `esc${motor + 1}.current`, `esc${motor + 1}.voltage`,
  ]).flat()]
  for (const fixture of [
    { views: { activeView: 'esc' }, top: ['esc1.rpm'], bottom: ['cmd.pwm1', 'mag.raw_total', 'bat1.current'] },
    { views: { views: ['cmd', 'cmd'] }, top: ['cmd.pwm1'], bottom: ['cmd.pwm1', 'mag.raw_total', 'esc1.rpm', 'bat1.current'] },
  ]) {
    const migrationContext = await browser.newContext()
    const migrationPage = await migrationContext.newPage()
    const migrationErrors = []
    migrationPage.on('pageerror', (error) => migrationErrors.push(String(error)))
    try {
      await migrationPage.addInitScript((preferences) => {
        localStorage.setItem('maggcs.actuator.scope', JSON.stringify(preferences))
      }, {
        checked: ['fwd.cmd1', 'mag.raw_total', 'esc1.rpm', 'bat1.current'],
        seen: legacySeen, windowS: 60, listWidth: 424, colors: { 'cmd.pwm1': '#4EA1FF' }, ...fixture.views,
      })
      await migrationPage.goto(url, { waitUntil: 'networkidle' })
      const restored = await migrationPage.evaluate(() => JSON.parse(localStorage.getItem('maggcs.actuator.scope')))
      assert.deepEqual(restored.panes.top.checked, fixture.top)
      assert.deepEqual(restored.panes.bottom.checked, fixture.bottom)
      assert.equal(restored.panes.top.checked.includes('cmd.pwm2'), false, 'old deliberate unchecks are not auto-selected')
      assert.equal(restored.windowS, 60)
      assert.equal(restored.listWidth, 424)
      assert.equal(restored.colors['cmd.pwm1'], '#4ea1ff')
      assert.equal(await migrationPage.locator('.scope-pane').count(), 2)
      assert.deepEqual(migrationErrors, [])
    } finally {
      await migrationContext.close()
    }
  }
  const corruptContext = await browser.newContext()
  const corruptPage = await corruptContext.newPage()
  const corruptErrors = []
  corruptPage.on('pageerror', (error) => corruptErrors.push(String(error)))
  try {
    await corruptPage.addInitScript(() => localStorage.setItem('maggcs.actuator.scope', 'null'))
    await corruptPage.goto(url, { waitUntil: 'networkidle' })
    assert.equal(await corruptPage.getByLabel('Time window', { exact: true }).inputValue(), '30')
    assert.equal(await corruptPage.locator('.scope-pane').count(), 2)
    assert.deepEqual(corruptErrors, [])
  } finally {
    await corruptContext.close()
  }
  console.log('PASS: single/two-view preference migration preserves selections, hidden groups, aliases, colors and width; corrupt storage recovers')
  assert.deepEqual(errors, [])
  console.log('PASS: start/stop gestures, tracking, persistent errors, capture/log controls, sidebar alarm, three viewport layouts and display preference synchronization')
} finally {
  await browser.close()
}
