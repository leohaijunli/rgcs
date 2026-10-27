// Browser acceptance for owned Mission IPC, cancellation and editor/target isolation.
import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shots] = process.argv.slice(2)
if (!url || !shots) throw new Error('usage: check-mission-ownership.mjs <vite-url> <screenshot-dir>')
let failed = 0
const check = (condition, label) => { console.log(`${condition ? 'PASS' : 'FAIL'} ${label}`); if (!condition) failed += 1 }
const browser = await chromium.launch({ headless: true, args: ['--disable-gpu', '--disable-dev-shm-usage'] })
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
const errors = []
page.on('pageerror', (error) => errors.push(String(error)))
page.on('console', (message) => { if (message.type() === 'error') errors.push(message.text()) })
await page.addInitScript(() => {
  localStorage.setItem('maggcs.autoconnect', 'false')
  const handlers = {}
  let eventId = 0
  let callbackId = 0
  let sessionId = 0
  let operationId = 0
  let revision = 1
  const sessions = new Map()
  const pending = []
  let fcItems = []
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b)
  const target = () => ({ vehicle_id: 'mission-browser-vehicle', session_id: `session-${revision}`, link_id: '1', generation: String(revision), mavlink_target: { system_id: 42, component_id: 1 } })
  const emit = (event, payload) => {
    const callback = window[`__missionCallback_${handlers[event]}`]
    if (typeof callback === 'function') callback({ event, id: ++eventId, payload })
  }
  const status = (entry, phase = 'running') => ({ operation: structuredClone(entry.operation), request_id: entry.context.request_id,
    resources: ['mission_transfer'], phase, cleanup: 'not_required', outcome: null, stop_reason: null, remaining_budget_ms: 30000 })
  window.__missionCalls = []
  window.__missionCancelCalls = []
  window.__delayMissionCancel = true
  window.__missionPending = () => pending.map((entry) => ({ kind: entry.kind, input: entry.input, context: entry.context, operation: entry.operation }))
  window.__missionEmit = emit
  window.__missionReply = (kind, result = null, operation = null) => {
    const index = pending.findIndex((entry) => entry.kind === kind)
    if (index < 0) throw new Error(`no pending ${kind}`)
    const [entry] = pending.splice(index, 1)
    if (!result) {
      if (kind === 'upload') { fcItems = structuredClone(entry.input.items); result = { kind: 'uploaded', count: fcItems.length } }
      else if (kind === 'download') result = { kind: 'downloaded', items: structuredClone(fcItems) }
      else if (kind === 'clear') { fcItems = []; result = { kind: 'cleared' } }
      else result = { kind: 'current', seq: entry.input.seq }
    }
    entry.resolve({ operation: operation ?? entry.operation, result })
  }
  window.__missionError = (kind, error) => {
    const index = pending.findIndex((entry) => entry.kind === kind)
    if (index < 0) throw new Error(`no pending ${kind}`)
    const [entry] = pending.splice(index, 1)
    entry.reject(error)
  }
  window.__missionProgress = (sent, total) => {
    const entry = pending.find((entry) => entry.kind === 'upload' || entry.kind === 'download')
    emit('mission_progress', { operation: entry.operation, request_id: entry.context.request_id, sent, total })
  }
  window.__missionReplaceTarget = (next, linkEvent = true) => {
    revision = next
    if (linkEvent) {
      emit('link', { link_state: 'connected', endpoint: 'fixture', fc_alive: false })
      emit('link', { link_state: 'connected', endpoint: 'fixture', fc_alive: true })
    }
  }
  window.__missionCurrent = (seq, observedTarget = target()) => emit('mission_current', { target: observedTarget, seq, received_at_ms: Date.now() })
  window.__missionFeed = () => {
    const at = Date.now()
    emit('telemetry', {
      last_heartbeat_at_ms: at, heartbeat: { system_id: 42, component_id: 1, vehicle_type: 'quadrotor', autopilot: 'px4',
        base_mode: { custom_mode_enabled: true, safety_armed: false }, custom_mode: 4 << 16 | 3 << 24, mode_label: 'Hold', flight_state: 'standby', mavlink_version: 3 },
      global_position: { time_boot_ms: 1, latitude_deg: 49.25, longitude_deg: -123.1, altitude: { datum: 'AMSL_EGM96', meters: 50 },
        relative_alt_m: 10, velocity: { x_m_s: 0, y_m_s: 0, z_m_s: 0 }, heading_deg: 0 },
      attitude: { roll_deg: 0, pitch_deg: 0, yaw_deg: 0 }, gps: { fix_type: 'RTK_FIXED', satellites_visible: 18 },
      sys_status: { sensors: {}, battery_voltage_mv: 16000, battery_current_ma: 0, battery_remaining_percent: 80 }, battery: null,
      home: { latitude_deg: 49.25, longitude_deg: -123.1, altitude: { datum: 'AMSL_EGM96', meters: 40 } },
      extended_sys_state: { landed_state: 'on_ground', vtol_state: 'undefined' }, estimator: null, vibration: null, rc_channels: null, radio: null,
      field_ages: { heartbeat_at_ms: at, global_position_at_ms: at, gps_at_ms: at, sys_status_at_ms: at, attitude_at_ms: at,
        home_at_ms: at, extended_sys_state_at_ms: at, battery_at_ms: null, estimator_at_ms: null, vibration_at_ms: null, rc_channels_at_ms: null, radio_at_ms: null },
    })
  }
  window.__TAURI_INTERNALS__ = {
    transformCallback: (callback) => { const id = ++callbackId; window[`__missionCallback_${id}`] = callback; return id },
    invoke: async (command, args = {}) => {
      if (command === 'plugin:event|listen') { handlers[args.event] = args.handler; return ++eventId }
      if (command.startsWith('plugin:event|')) return null
      if (command === 'runtime_targets') return [{ target: target(), link_id: '1', availability: 'available', roles: ['primary'],
        transport_owner: 'primary', endpoint: 'fixture', parameter_service_available: true, heartbeat_age_ms: 0 }]
      if (command === 'runtime_client_attach') {
        const session = { owner: { client_id: 'mission-main-window', instance_id: `instance-${++sessionId}` }, module_id: args.moduleId, target: structuredClone(args.target) }
        sessions.set(args.moduleId, session)
        return structuredClone(session)
      }
      if (command === 'runtime_client_validate') {
        if (!same(sessions.get(args.session.module_id), args.session)) throw { code: 'not_owner' }
        if (!same(args.session.target, target())) throw { code: 'runtime', error: { code: 'target_changed' } }
        return null
      }
      if (command === 'runtime_client_detach') { if (same(sessions.get(args.session.module_id), args.session)) sessions.delete(args.session.module_id); return null }
      if (command === 'mission_cancel') {
        window.__missionCancelCalls.push(structuredClone(args))
        const entry = pending.find((entry) => same(entry.operation, args.operation) && same(entry.context.session, args.session))
        if (!entry) throw { code: 'not_owner' }
        emit('mission_operation', status(entry, 'cancel_requested'))
        if (!window.__delayMissionCancel) window.__missionError(entry.kind, { code: 'reconciliation_required', operation: entry.operation, reason: 'Local retries retired; FC mission requires read-back' })
        return status(entry, 'cancel_requested')
      }
      if (['mission_upload', 'mission_download', 'mission_clear', 'mission_set_current'].includes(command)) {
        window.__missionCalls.push({ command, ...structuredClone(args) })
        return new Promise((resolve, reject) => {
          const context = args.request.context ?? args.request
          const entry = { kind: command.slice('mission_'.length), input: structuredClone(args.request), context: structuredClone(context), resolve, reject,
            operation: { id: `operation-${++operationId}`, owner: structuredClone(context.session.owner), target: structuredClone(context.session.target) } }
          pending.push(entry)
          emit('mission_operation', status(entry))
        })
      }
      if (command === 'preflight_checklist') return { items: [{ id: 'link_fresh', group: 'link', status: 'pass', detail: 'Current link', fix_hint: null }] }
      if (command === 'alert_snapshot' || command === 'enumerate_devices' || command === 'flight_mode_table') return []
      return null
    },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
})
await page.goto(`${url}?nomap`, { waitUntil: 'networkidle', timeout: 60000 })
await page.evaluate(async () => {
  window.__mgProject?.setState({ project: { version: 1, name: 'Mission ownership', crs: 'EPSG:4326', datum: 'cgvd2013', areas: [], settings: null } })
  const { useMissionStore } = await import('/src/stores/mission.ts')
  window.__missionStore = useMissionStore
  window.__missionFeed()
  window.__missionFeedTimer = setInterval(window.__missionFeed, 200)
  window.__missionEmit('link', { link_state: 'connected', endpoint: 'fixture', fc_alive: true })
})
await page.waitForTimeout(300)
const targetConfirm = page.getByTestId('target-change-confirm')
if (await targetConfirm.isVisible()) await targetConfirm.click()
await mkdir(shots, { recursive: true })
async function wait(predicate) {
  for (let index = 0; index < 50; index += 1) { if (await predicate()) return true; await page.waitForTimeout(100) }
  return false
}
const pending = (kind) => page.evaluate((kind) => window.__missionPending().find((entry) => entry.kind === kind) ?? null, kind)
const store = () => page.evaluate(() => ({ busy: window.__missionStore.getState().busy, fcMatches: window.__missionStore.getState().fcMatches,
  dirty: window.__missionStore.getState().dirty, baseline: window.__missionStore.getState().lastSyncedHash,
  operation: window.__missionStore.getState().operation, reconciliation: window.__missionStore.getState().requiresReconciliation,
  currentSeq: window.__missionStore.getState().currentSeq, waypoints: window.__missionStore.getState().waypoints }))
async function shot(name) { await page.screenshot({ path: join(shots, name) }) }
async function syncCard() {
  const card = page.locator('button[aria-expanded]', { hasText: 'Sync' }).first()
  if (await card.getAttribute('aria-expanded') !== 'true') await card.click()
  await page.getByTestId('mission-upload').waitFor()
}
await page.getByTestId('workflow-open').click()
await page.getByTestId('workflow-stage-plan').click()
await page.evaluate(() => window.__missionStore.getState().setWaypoints([0, 1].map((index) => ({
  position: { latitude_deg: 49.25 + index * 0.001, longitude_deg: -123.1 }, altitude: { datum: 'AMSL_EGM96', meters: 50 },
  command: 16, params: [0, 0, 0, 0], autocontinue: true, locked: false,
}))))
await syncCard()
await page.getByTestId('mission-upload').click()
check(await wait(async () => Boolean(await pending('upload'))), 'Upload issues an owned request from the Sync UI')
await page.evaluate(() => window.__missionProgress(1, 2))
check((await page.getByTestId('task-progress').innerText()).includes('1/2'), 'owned progress displays its counters')
check(await page.getByTestId('mission-download').isDisabled(), 'another transfer is gated while upload owns the protocol')
check(await page.getByTestId('task-cancel').isEnabled(), 'owned local cancellation is available')
check((await page.getByTestId('mission-operation-id').innerText()).includes('generation 1'), 'the UI retains exact operation/generation evidence')
await shot('mission-upload-progress.png')
await page.evaluate(() => window.__missionReply('upload'))
check(await wait(async () => Boolean(await pending('download'))), 'accepted upload starts a separate owned read-back')
check((await store()).fcMatches === null, 'an upload ACK does not claim verified FC sync')
await shot('mission-verifying.png')
await page.evaluate(() => window.__missionReply('download'))
check(await wait(async () => (await store()).fcMatches === true), 'matching read-back verifies the captured wire snapshot')
await shot('mission-verified.png')

await page.getByTestId('mission-upload').click()
await wait(async () => Boolean(await pending('upload')))
const original = await pending('upload')
await page.evaluate(() => window.__missionStore.getState().updateAltitude(0, 133))
check(original.input.items[0].z !== 133, 'the clicked upload snapshot is independent of later edits')
await page.evaluate(() => window.__missionReply('upload'))
await wait(async () => Boolean(await pending('download')))
await page.evaluate(() => window.__missionReply('download'))
await wait(async () => !(await store()).busy)
check((await store()).waypoints[0].altitude.meters === 133 && (await store()).dirty, 'read-back leaves the newer editor and its dirty state intact')
check((await store()).fcMatches !== true, 'later edits are not falsely verified as the uploaded mission')
await shot('mission-editor-changed.png')

await page.getByTestId('mission-upload').click()
await wait(async () => Boolean(await pending('upload')))
const cancelled = await pending('upload')
await page.getByTestId('task-cancel').click()
check(await wait(async () => page.evaluate(() => window.__missionCancelCalls.length === 1)), 'Cancel calls the exact owned cancellation IPC')
const cancellation = await page.evaluate(() => window.__missionCancelCalls[0])
check(JSON.stringify(cancellation.operation) === JSON.stringify(cancelled.operation), 'Cancel carries the original OperationRef')
check(JSON.stringify(cancellation.session) === JSON.stringify(cancelled.context.session), 'Cancel carries the original native-issued session')
check((await page.getByTestId('task-progress').innerText()).includes('Cancelling local mission retries'), 'local retry retirement remains distinct from FC outcome')
await shot('mission-cancelling.png')
await page.evaluate(() => window.__missionError('upload', { code: 'reconciliation_required', reason: 'Local retries retired; FC mission requires read-back' }))
check(await wait(async () => !(await store()).busy), 'cancelled transfer retires its busy UI state')
check((await store()).reconciliation && (await store()).waypoints.length === 2, 'uncertain cancellation preserves the editor and requests FC reconciliation')
await shot('mission-cancelled-uncertain.png')

await page.evaluate(() => window.__missionReplaceTarget(2))
check(await wait(async () => Boolean(await pending('download'))), 'target replacement starts a fresh owned mission read-back')
await page.evaluate(() => window.__missionReply('download'))
await wait(async () => !(await store()).busy)
await page.getByTestId('mission-more').click()
const beforeClear = await page.evaluate(() => window.__missionCalls.length)
await page.getByTestId('mission-clear-fc').click()
check(await page.evaluate(() => window.__missionCalls.length) === beforeClear, 'the first destructive click cannot clear the FC mission')
await page.getByTestId('mission-clear-fc').click()
check(await wait(async () => Boolean(await pending('clear'))), 'the confirmed clear has its own typed request')
await page.evaluate(() => window.__missionError('clear', { code: 'protocol', error: { code: 'ack_denied', ack_type: 2 } }))
check(await wait(async () => (await page.getByTestId('mission-error').innerText()).includes('rejected')), 'a denied clear keeps its real FC verdict visible')
await shot('mission-clear-denied.png')

await page.getByTestId('mission-download').click()
await wait(async () => Boolean(await pending('download')))
await page.evaluate(() => window.__missionStore.getState().updatePosition(0, 49.27, -123.1))
await page.evaluate(() => window.__missionReply('download'))
check(await wait(async () => !(await store()).busy), 'download finishes without clobbering an intervening edit')
check((await store()).waypoints[0].position.latitude_deg === 49.27, 'newer local coordinates survive old downloaded data')
check((await page.getByTestId('mission-error').innerText()).includes('Local plan changed'), 'the discarded editor overwrite is explained')
await shot('mission-download-not-applied.png')

await page.getByTestId('mission-upload').click()
await wait(async () => Boolean(await pending('upload')))
const stale = await pending('upload')
await page.evaluate(() => window.__missionReplaceTarget(3))
check(await wait(async () => Boolean(await pending('download'))), 'replacement read-back owns the protocol before another upload')
await page.evaluate(() => window.__missionReply('download'))
await wait(async () => !(await store()).busy)
await page.getByTestId('mission-upload').click()
await wait(async () => page.evaluate(() => window.__missionPending().filter((entry) => entry.kind === 'upload').length === 2))
const fresh = await page.evaluate(() => window.__missionPending().filter((entry) => entry.kind === 'upload')[1])
await page.evaluate(() => window.__missionReply('upload'))
await page.waitForTimeout(200)
check((await store()).busy && (await store()).operation.id === fresh.operation.id, 'an old-target reply cannot settle the newer transfer')
await page.evaluate((entry) => window.__missionEmit('mission_progress', { operation: entry.operation, request_id: entry.context.request_id, sent: 99, total: 99 }), stale)
check(!(await page.getByTestId('task-progress').innerText()).includes('99/99'), 'old operation progress has no authority over the replacement request')
await shot('mission-stale-reply.png')
await page.evaluate(() => window.__missionReply('upload'))
await wait(async () => Boolean(await pending('download')))
await page.evaluate(() => window.__missionReply('download'))
await wait(async () => !(await store()).busy)
check((await store()).fcMatches === true, 'only the fresh target/read-back establishes sync')
await page.evaluate((target) => window.__missionCurrent(99, target), stale.context.session.target)
await page.waitForTimeout(200)
check((await store()).currentSeq !== 99, 'old-target passive current observation is discarded')

const waypoints = page.locator('button[aria-expanded]', { hasText: 'Waypoints' }).first()
await waypoints.click()
await page.evaluate(() => window.__missionStore.getState().select(1))
await page.getByRole('button', { name: 'Set current', exact: true }).click()
check(await wait(async () => Boolean(await pending('set_current'))), 'Set current uses the owned request from the waypoint editor')
await page.evaluate(() => window.__missionCurrent(0))
await page.waitForTimeout(100)
check((await store()).busy, 'a different passive current sequence cannot complete Set current')
await page.evaluate(() => { window.__missionCurrent(1); window.__missionReply('set_current') })
check(await wait(async () => !(await store()).busy && (await store()).currentSeq === 1), 'matching current reply/observation confirms the requested sequence')
const currentBox = await page.getByTestId('mission-current-marker').boundingBox()
check(currentBox !== null && currentBox.x >= 0 && currentBox.x + currentBox.width <= 1440, 'the active waypoint marker stays fully inside the editor viewport')
await shot('mission-set-current-confirmed.png')
await syncCard()
await page.getByTestId('mission-upload').click()
await wait(async () => Boolean(await pending('upload')))
await page.evaluate(() => { window.__missionReplaceTarget(4, false); window.__missionReply('upload') })
check(await wait(async () => !(await store()).busy && (await store()).baseline === null), 'native revalidation rejects silent generation replacement')
await shot('mission-silent-target-loss.png')
const all = await page.evaluate(() => window.__missionCalls)
const contexts = all.map((entry) => entry.request.context ?? entry.request)
check(contexts.every((context) => context.session.module_id === 'mission'), 'every effect uses the mission module attachment')
check(new Set(contexts.map((context) => context.request_id)).size === contexts.length, 'every effect/read-back has a distinct request ID')
check(errors.length === 0, 'no page errors during Mission ownership regression')
await browser.close()
if (errors.length) console.error(errors)
if (failed || errors.length) process.exit(1)
console.log('check:mission-ownership OK')
