// AF-02b.3 / AF-02b.4a real Parameters UI, owned parameters and reboot commands.
// Usage: node scripts/check-params-ownership.mjs [url] [screenshots]
// Protocol/actual cancellation is separately covered by native PTY fixtures.
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const require = createRequire(import.meta.url)
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright')
const url = process.argv[2] ?? 'http://localhost:5199/params.html'
const shots = process.argv[3]
const browser = await chromium.launch({ headless: true })
const context = await browser.newContext({ viewport: { width: 1366, height: 768 } })
const pages = new Map()
const sessions = new Map()
const operations = new Map()
const calls = []
const errors = []
let generation = 1
let sequence = 0
let holdWrite = false
let pendingWrite
let rebootMode = 'denied'
const pendingReboots = []
let entry = { name: 'COM_RC_IN_MODE', index: 0, ptype: 'i32', value: { int: 0 } }
const target = () => ({ vehicle_id: 'vehicle-a', session_id: `target-session-${generation}`, link_id: '1', generation: String(generation), mavlink_target: { system_id: 42, component_id: 1 } })
const link = (alive) => ({ link_state: 'connected', endpoint: 'isolated Parameters IPC', fc_alive: alive })
const until = async (read, label) => {
  const end = Date.now() + 8000
  while (!await read()) { assert.ok(Date.now() < end, label); await new Promise((r) => setTimeout(r, 30)) }
}
const emit = async (event, payload) => {
  for (const page of pages.values()) if (!page.isClosed()) await page.evaluate(({ event, payload }) => window.__deliver(event, payload), { event, payload })
}
const parameterEvent = (payload, ref = target(), id = null) => emit('params_event', { target: ref, operation_id: id, ...payload })
function authorize(name, session) {
  const current = sessions.get(name)
  if (!current || JSON.stringify(current) !== JSON.stringify(session)) throw { code: 'operation', error: { code: 'not_owner' } }
  if (session.target.generation !== String(generation)) throw { code: 'operation', error: { code: 'target_changed' } }
}
const statusFor = (operation, phase, outcome = null) => ({ operation, request_id: `request-${operation.id}`, resources: ['parameter_transfer'], phase, cleanup: phase === 'finished' ? 'confirmed' : 'not_required', outcome, stop_reason: null, remaining_budget_ms: 5000 })

async function open(name) {
  const page = await context.newPage()
  pages.set(name, page)
  page.on('pageerror', (error) => errors.push(String(error)))
  await page.exposeFunction('__nativeInvoke', async (command, args = {}) => {
    calls.push({ client: name, command, args })
    if (command === 'config_load') return null
    if (command === 'alert_snapshot') return []
    if (command === 'link_status') return link(true)
    if (command === 'params_fc_version') return '1.17.0'
    if (command === 'runtime_targets') return [{ link_id: '1', target: target(), availability: 'available', heartbeat_age_ms: 0,
      parameter_service_available: true, roles: ['primary'], transport_owner: 'primary', endpoint: 'isolated Parameters IPC' }]
    if (command === 'runtime_client_attach') {
      const session = { owner: { client_id: `client-${name}`, instance_id: `instance-${name}-${generation}` }, target: args.target, module_id: args.moduleId }
      sessions.set(name, session)
      return session
    }
    if (command === 'runtime_client_validate') { authorize(name, args.session); return null }
    if (command === 'runtime_client_detach') {
      if (JSON.stringify(sessions.get(name)) === JSON.stringify(args.session)) sessions.delete(name)
      return null
    }
    if (command === 'params_read') {
      authorize(name, args.request.context.session)
      assert.ok(args.request.context.request_id)
      return args.request.name === entry.name ? structuredClone(entry) : null
    }
    if (command === 'params_pull') {
      authorize(name, args.request.session)
      assert.ok(args.request.request_id)
      const operation = { id: `operation-${++sequence}`, owner: args.request.session.owner, target: args.request.session.target }
      operations.set(operation.id, statusFor(operation, 'running'))
      await parameterEvent({ kind: 'pull_started', total: 2 }, operation.target, operation.id)
      await parameterEvent({ kind: 'progress', received: 1, total: 2 }, operation.target, operation.id)
      return operation
    }
    if (command === 'params_operation_status') {
      authorize(name, args.session)
      const result = operations.get(args.operation.id)
      if (result.operation.owner.client_id !== args.session.owner.client_id) throw { code: 'operation', error: { code: 'not_owner' } }
      return result
    }
    if (command === 'params_cancel') {
      authorize(name, args.session)
      const current = operations.get(args.operation.id)
      if (!current || JSON.stringify(current.operation) !== JSON.stringify(args.operation)
        || args.operation.owner.client_id !== args.session.owner.client_id) throw { code: 'operation', error: { code: 'not_owner' } }
      const result = statusFor(current.operation, 'finished', { kind: 'cancelled' })
      operations.set(result.operation.id, result)
      await emit('params_operation', result)
      return result
    }
    if (command === 'params_set') {
      const request = args.request
      authorize(name, request.context.session)
      assert.ok(request.context.request_id)
      const operation = { id: `operation-${++sequence}`, owner: request.context.session.owner, target: request.context.session.target }
      const result = { operation, status: 'confirmed', actual: request.value, requested: null }
      if (holdWrite) return new Promise((resolve) => { pendingWrite = { request, result, resolve } })
      entry.value = request.value
      await parameterEvent({ kind: 'external_change', name: entry.name, ptype: entry.ptype, value: entry.value }, operation.target, operation.id)
      return result
    }
    if (command === 'params_reboot') {
      authorize(name, args.request.session)
      assert.ok(args.request.request_id)
      const operation = { id: `operation-${++sequence}`, owner: args.request.session.owner, target: args.request.session.target }
      const result = { operation, result: rebootMode === 'hold' ? 'accepted' : rebootMode }
      if (rebootMode === 'hold') return new Promise((resolve) => { pendingReboots.push({ request: args.request, result, resolve }) })
      if (rebootMode === 'uncertain') throw { code: 'reconciliation_required', operation, reason: 'Reboot outcome is unconfirmed; reconcile the target before retrying' }
      return result
    }
    return null
  })
  await page.addInitScript(() => {
    const callbacks = new Map(), listeners = new Map()
    let id = 0
    window.__deliver = (event, payload) => {
      for (const [key, listener] of listeners) if (listener.event === event) callbacks.get(listener.handler)?.({ id: key, event, payload })
    }
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_event, key) => listeners.delete(key) }
    window.__TAURI_INTERNALS__ = {
      transformCallback(callback) { callbacks.set(++id, callback); return id },
      unregisterCallback(key) { callbacks.delete(key) },
      async invoke(command, args) {
        if (command === 'plugin:event|listen') { listeners.set(++id, args); return id }
        if (command === 'plugin:event|unlisten') { listeners.delete(args.eventId); return null }
        const result = await window.__nativeInvoke(command, args)
        if (command === 'params_reboot') window.__rebootReplyCount = (window.__rebootReplyCount ?? 0) + 1
        return result
      },
    }
  })
  await page.goto(url, { waitUntil: 'domcontentloaded' })
  await page.locator('.p-row').first().waitFor()
  await until(() => page.getByRole('button', { name: 'Pull all', exact: true }).isEnabled(), 'target client ready')
  return page
}

async function stage(page) {
  await page.locator('.p-search').fill(entry.name)
  await page.locator('.p-row').first().click()
  await until(() => page.locator('.p-edit-select').inputValue().then((value) => value === '0'), 'current value read')
  await page.locator('.p-edit-select').selectOption('1')
  await page.getByRole('button', { name: 'Stage', exact: true }).click()
}

try {
  if (shots) await mkdir(shots, { recursive: true })
  const a = await open('a')
  const b = await open('b')
  await b.getByRole('button', { name: 'Pull all', exact: true }).click()
  await until(() => b.getByRole('button', { name: 'Cancel', exact: true }).isEnabled(), 'b owns pull')
  await until(() => a.getByRole('button', { name: 'Cancel', exact: true }).isDisabled(), 'a cannot cancel observed b pull')
  assert.equal(calls.filter((call) => call.client === 'a' && call.command === 'params_cancel').length, 0)
  if (shots) await a.screenshot({ path: join(shots, 'params-observer-cancel-disabled.png') })
  const bOperation = [...operations.values()][0].operation
  const aSession = sessions.get('a')
  const forged = await a.evaluate(async ({ session, operation }) => {
    try { await window.__TAURI_INTERNALS__.invoke('params_cancel', { session, operation }); return null }
    catch (error) { return error }
  }, { session: aSession, operation: bOperation })
  assert.equal(forged.error.code, 'not_owner')
  await a.close()
  sessions.delete('a')
  assert.equal(operations.get(bOperation.id).phase, 'running')
  await b.getByRole('button', { name: 'Cancel', exact: true }).click()
  await until(() => b.locator('.p-status').innerText().then((text) => text.includes('cancelled')), 'owned cancel finished')
  const cancel = calls.find((call) => call.client === 'b' && call.command === 'params_cancel')
  assert.equal(cancel.args.operation.id, bOperation.id)
  assert.equal(cancel.args.session.owner.client_id, 'client-b')
  if (shots) await b.screenshot({ path: join(shots, 'params-owned-cancel-finished.png') })

  const reopened = await open('a-reopened')
  await stage(reopened)
  holdWrite = true
  await reopened.getByRole('button', { name: 'Apply 1', exact: true }).click()
  await until(() => Boolean(pendingWrite), 'old write pending')
  assert.equal(pendingWrite.request.context.session.target.generation, '1')
  generation = 2
  await emit('link', link(false))
  await emit('link', link(true))
  await until(() => Promise.resolve(sessions.get('a-reopened')?.target.generation === '2'), 'new target session ready')
  // Deliberately deliver an old software result even though the native driver
  // would retain cancellation uncertainty; the view must not apply it to gen 2.
  pendingWrite.resolve(pendingWrite.result)
  await until(() => reopened.getByRole('button', { name: 'Apply 1', exact: true }).isEnabled(), 'old result kept current staged draft')
  await parameterEvent({ kind: 'external_change', name: entry.name, ptype: entry.ptype, value: { int: 99 } }, { ...target(), session_id: 'target-session-1', generation: '1' })
  await until(() => reopened.locator('.p-edit-select').inputValue().then((value) => value === '0'), 'old target event ignored')
  if (shots) await reopened.screenshot({ path: join(shots, 'params-stale-write-retains-staging.png') })
  holdWrite = false
  pendingWrite = undefined
  await reopened.getByRole('button', { name: 'Apply 1', exact: true }).click()
  await until(() => reopened.locator('.p-tray').count().then((count) => count === 0), 'current scoped write applied')
  const writes = calls.filter((call) => call.client === 'a-reopened' && call.command === 'params_set')
  assert.deepEqual(writes.map((call) => call.args.request.context.session.target.generation), ['1', '2'])
  assert.notEqual(writes[0].args.request.context.request_id, writes[1].args.request.context.request_id)

  entry = { name: 'USB_MAV_MODE', index: 1, ptype: 'i32', value: { int: 0 } }
  await stage(reopened)
  await reopened.getByRole('button', { name: 'Apply 1', exact: true }).click()
  const rebootButton = reopened.getByRole('button', { name: 'Reboot FC', exact: true })
  await until(() => rebootButton.isEnabled(), 'applied parameter requires reboot')
  await rebootButton.click()
  await until(() => reopened.locator('.p-status').innerText().then((text) => text.includes('reboot (denied)')), 'FC denial stays a distinct verdict')
  if (shots) await reopened.screenshot({ path: join(shots, 'params-reboot-denied.png') })

  rebootMode = 'uncertain'
  await rebootButton.click()
  await until(() => reopened.locator('.p-status').innerText().then((text) => text.includes('Reboot outcome is unconfirmed')), 'uncertainty is not reboot success')
  if (shots) await reopened.screenshot({ path: join(shots, 'params-reboot-uncertain.png') })

  rebootMode = 'hold'
  await rebootButton.click()
  await until(() => Promise.resolve(pendingReboots.length === 1), 'old reboot reply held')
  const oldReboot = pendingReboots[0]
  assert.equal(oldReboot.request.session.target.generation, '2')
  entry.value = { int: 0 }
  generation = 3
  await emit('link', link(false))
  await emit('link', link(true))
  await until(() => Promise.resolve(sessions.get('a-reopened')?.target.generation === '3'), 'reboot retargeted client ready')
  await until(() => reopened.locator('.p-reboot-banner').count().then((count) => count === 0), 'old target reboot requirement cleared')
  oldReboot.resolve(oldReboot.result)
  await reopened.waitForFunction(() => window.__rebootReplyCount === 2)
  await reopened.evaluate(() => new Promise(requestAnimationFrame))
  assert.ok(!(await reopened.locator('.p-status').innerText()).includes('Reboot accepted'))
  assert.equal(calls.filter((call) => call.command === 'params_reboot').length, 3, 'target recovery does not replay reboot')
  if (shots) await reopened.screenshot({ path: join(shots, 'params-stale-reboot-ignored.png') })

  await stage(reopened)
  await reopened.getByRole('button', { name: 'Apply 1', exact: true }).click()
  await until(() => rebootButton.isEnabled(), 'new target reboot requirement')
  await rebootButton.click()
  await until(() => Promise.resolve(pendingReboots.length === 2), 'new owned reboot')
  const currentReboot = pendingReboots[1]
  assert.equal(currentReboot.request.session.target.generation, '3')
  assert.notEqual(currentReboot.request.request_id, oldReboot.request.request_id)
  currentReboot.resolve(currentReboot.result)
  await until(() => reopened.locator('.p-status').innerText().then((text) => text.includes('Reboot accepted')), 'current owned acceptance shown')
  if (shots) await reopened.screenshot({ path: join(shots, 'params-reboot-accepted.png') })
  const reboots = calls.filter((call) => call.command === 'params_reboot')
  assert.deepEqual(reboots.map((call) => call.args.request.session.target.generation), ['2', '2', '2', '3'])
  assert.equal(new Set(reboots.map((call) => call.args.request.request_id)).size, reboots.length)
  assert.deepEqual(errors, [])
  console.log('PASS: Parameters owner/close isolation, target-bound writes/reboot, distinct denial/uncertainty, stale result rejection and no reconnect replay')
} finally {
  await browser.close()
}
