// Real React/store ownership acceptance; only the native boundary is simulated.
import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
const [url, shots] = process.argv.slice(2)
if (!url || !shots) throw new Error('usage: check-shell-logs.mjs <vite-url> <screenshots>')
let failed = 0
const check = (condition, label) => { console.log(`${condition ? 'PASS' : 'FAIL'} ${label}`); if (!condition) failed += 1 }
const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
const errors = []
page.on('pageerror', (error) => errors.push(String(error)))
let accept = true
const dialogs = []
page.on('dialog', async (dialog) => { dialogs.push(dialog.message()); if (accept) await dialog.accept(); else await dialog.dismiss() })
await page.addInitScript(() => {
  localStorage.setItem('maggcs.autoconnect', 'false')
  localStorage.removeItem('maggcs.rightWidth')
  const handlers = {}
  const sessions = new Map()
  const pending = []
  let callbackId = 0, operationId = 0, instanceId = 0, sequence = 0, generation = 1
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b)
  const target = () => ({ vehicle_id: 'logs-vehicle', session_id: `vehicle-${generation}`, link_id: '1', generation: String(generation), mavlink_target: { system_id: 42, component_id: 1 } })
  const emit = (event, payload) => {
    for (const handler of handlers[event] ?? []) window[`__shellCallback_${handler}`]?.({ event, id: 1, payload })
  }
  const status = (entry, finished = false) => ({ operation: entry.operation, request_id: entry.context.request_id,
    resources: ['log_transfer'], phase: finished ? 'finished' : 'running', cleanup: 'confirmed', outcome: finished ? { kind: 'succeeded' } : null,
    stop_reason: null, remaining_budget_ms: 10000 })
  window.__calls = []
  window.__savePath = null
  window.__emit = emit
  window.__pending = () => pending.map((entry) => ({ command: entry.command, context: entry.context, operation: entry.operation, input: entry.input }))
  window.__reply = (command, result, wrongOperation = null) => {
    const index = pending.findIndex((entry) => entry.command === command)
    if (index < 0) throw new Error(`no pending ${command}`)
    const [entry] = pending.splice(index, 1)
    emit('logs_operation', status(entry, true))
    entry.resolve({ operation: wrongOperation ?? entry.operation, result })
  }
  window.__fail = (command, error) => {
    const index = pending.findIndex((entry) => entry.command === command)
    const [entry] = pending.splice(index, 1)
    entry.reject(error)
  }
  window.__replace = () => {
    generation += 1
    emit('link', { link_state: 'connected', endpoint: 'fixture', fc_alive: false })
    emit('link', { link_state: 'connected', endpoint: 'fixture', fc_alive: true })
  }
  window.__stealthReplace = () => { generation += 1 }
  window.__feed = () => {
    const at = Date.now()
    emit('telemetry', { heartbeat: { system_id: 42, component_id: 1, vehicle_type: 'quadrotor', autopilot: 'px4',
      base_mode: { safety_armed: false, custom_mode_enabled: true }, custom_mode: 4 << 16 | 3 << 24, mode_label: 'Hold', flight_state: 'standby', mavlink_version: 3 },
      global_position: { time_boot_ms: 1, latitude_deg: 49.25, longitude_deg: -123.1, altitude: { datum: 'AMSL_EGM96', meters: 50 }, relative_alt_m: 0, velocity: { x_m_s: 0, y_m_s: 0, z_m_s: 0 }, heading_deg: 0 },
      gps: { fix_type: 'RTK_FIXED', satellites_visible: 18 }, attitude: { roll_deg: 0, pitch_deg: 0, yaw_deg: 0 },
      sys_status: { sensors: {}, battery_voltage_mv: 16000, battery_current_ma: 0, battery_remaining_percent: 80 }, battery: null,
      extended_sys_state: { landed_state: 'on_ground', vtol_state: 'undefined' }, last_heartbeat_at_ms: at,
      field_ages: { heartbeat_at_ms: at, global_position_at_ms: at, attitude_at_ms: at, gps_at_ms: at, sys_status_at_ms: at, extended_sys_state_at_ms: at } })
  }
  window.__TAURI_INTERNALS__ = {
    transformCallback: (callback) => { const id = ++callbackId; window[`__shellCallback_${id}`] = callback; return id },
    invoke: async (command, args = {}) => {
      if (command === 'plugin:event|listen') { (handlers[args.event] ??= []).push(args.handler); return callbackId }
      if (command.startsWith('plugin:event|')) return null
      if (command === 'runtime_targets') return [{ target: target(), link_id: '1', availability: 'available', roles: ['primary'], transport_owner: 'primary', endpoint: 'fixture', parameter_service_available: true, heartbeat_age_ms: 0 }]
      if (command === 'runtime_client_attach') {
        window.__calls.push({ command, args: structuredClone(args) })
        const session = { owner: { client_id: 'main-window', instance_id: `instance-${++instanceId}` }, module_id: args.moduleId, target: args.target }
        sessions.set(args.moduleId, session)
        return structuredClone(session)
      }
      if (command === 'runtime_client_validate') {
        if (!same(args.session, sessions.get(args.session.module_id)) || !same(args.session.target, target())) throw { code: 'target_changed' }
        return null
      }
      if (command === 'runtime_client_detach') { if (same(args.session, sessions.get(args.session.module_id))) sessions.delete(args.session.module_id); return null }
      if (command === 'plugin:dialog|save') { window.__calls.push({ command }); return window.__savePath }
      if (command === 'console_open') {
        window.__calls.push({ command, args: structuredClone(args) }); sequence = 0
        window.__shellOperation = { id: `shell-${++operationId}`, owner: args.request.session.owner, target: args.request.session.target }
        return { operation: structuredClone(window.__shellOperation) }
      }
      if (command === 'console_send') {
        window.__calls.push({ command, args: structuredClone(args) })
        emit('console_output', { operation: args.request.operation, target: args.request.session.target, sequence: String(++sequence), received_at_ms: Date.now(), chunk: 'shell response\n' })
        return { operation: args.request.operation, bytes: new TextEncoder().encode(args.request.command).length }
      }
      if (command === 'console_close' || command === 'console_cancel') {
        window.__calls.push({ command, args: structuredClone(args) })
        const result = { operation: args.operation, phase: 'finished', cleanup: 'confirmed', outcome: { kind: 'succeeded' } }
        emit('console_operation', result)
        return result
      }
      if (command === 'logs_cancel') {
        window.__calls.push({ command, args: structuredClone(args) })
        const entry = pending.find((entry) => same(entry.operation, args.operation))
        if (!entry || !same(entry.context.session, args.session)) throw { code: 'not_owner' }
        window.__fail(entry.command, { code: 'protocol', error: { code: 'cancelled' } })
        return status(entry)
      }
      if (['logs_request_list', 'logs_download', 'logs_delete', 'logs_erase_all'].includes(command)) {
        window.__calls.push({ command, args: structuredClone(args) })
        return new Promise((resolve, reject) => {
          const context = args.request.context ?? args.request
          const entry = { command, context: structuredClone(context), input: structuredClone(args.request), resolve, reject,
            operation: { id: `logs-${++operationId}`, owner: structuredClone(context.session.owner), target: structuredClone(context.session.target) } }
          pending.push(entry)
          emit('logs_operation', status(entry))
        })
      }
      if (['enumerate_devices', 'alert_snapshot', 'flight_mode_table'].includes(command)) return []
      return null
    },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
})
try {
  await page.goto(`${url}?nomap`, { waitUntil: 'networkidle', timeout: 60000 })
  await page.evaluate(async () => {
    window.__mgProject?.setState({ project: { version: 1, name: 'Shell/Logs', crs: 'EPSG:4326', datum: 'cgvd2013', areas: [], settings: null } })
    window.__logs = (await import('/src/stores/logs.ts')).useLogsStore
    window.__console = (await import('/src/stores/console.ts')).useConsoleStore
    window.__feed()
    setInterval(window.__feed, 200)
    window.__emit('link', { link_state: 'connected', endpoint: 'fixture', fc_alive: true })
  })
  await page.waitForTimeout(300)
  const targetConfirm = page.getByTestId('target-change-confirm')
  if (await targetConfirm.isVisible()) await targetConfirm.click()
  await mkdir(shots, { recursive: true })
  await page.getByTestId('tools-open').click()
  await page.getByTestId('tool-logs').click()
  await page.getByTestId('right-dock').waitFor()
  check(await page.getByTestId('right-dock-logs').isVisible(), 'Logs opens in the right dock')
  const state = () => page.evaluate(() => ({ busy: window.__logs.getState().busy, entries: window.__logs.getState().entries,
    operation: window.__logs.getState().operation, error: window.__logs.getState().error, reconciliation: window.__logs.getState().reconciliation }))
  const calls = () => page.evaluate(() => window.__calls)
  const pending = (command) => page.evaluate((command) => window.__pending().find((entry) => entry.command === command), command)
  async function wait(predicate) { for (let i = 0; i < 50; i += 1) { if (await predicate()) return; await page.waitForTimeout(50) } throw new Error('fixture deadline') }
  const entry = { id: 0, size: 360, time_utc: 1700000000 }
  async function list(entries = [entry]) {
    await page.getByRole('button', { name: 'Refresh list', exact: true }).click()
    await wait(() => pending('logs_request_list'))
    await page.evaluate((entries) => window.__reply('logs_request_list', { kind: 'listed', entries }), entries)
    await wait(async () => !(await state()).busy)
  }
  await list()
  check((await state()).entries.length === 1, 'typed owned list renders')
  await page.screenshot({ path: join(shots, 'connected-list.png') })
  const beforePicker = (await calls()).filter((call) => call.command.startsWith('logs_') || call.command === 'runtime_client_attach').length
  await page.getByRole('button', { name: 'Download', exact: true }).click()
  await wait(async () => !(await state()).busy)
  check((await calls()).filter((call) => call.command.startsWith('logs_') || call.command === 'runtime_client_attach').length === beforePicker, 'cancelled picker starts zero operation/attachment')
  await page.evaluate(() => { window.__savePath = '/tmp/log0.ulg' })
  await page.getByRole('button', { name: 'Download', exact: true }).click()
  await wait(() => pending('logs_download'))
  const download = await pending('logs_download')
  check(download.input.save_path === '/tmp/log0.ulg' && download.input.entry.time_utc === entry.time_utc, 'download captures destination and full log identity')
  await page.evaluate((download) => window.__emit('logs_progress', { operation: download.operation, request_id: download.context.request_id, entry: download.input.entry, fetched: 180, total: 360 }), download)
  await page.screenshot({ path: join(shots, 'download-progress.png') })
  check((await state()).busy, 'progress does not report durable file completion')
  await page.evaluate(() => window.__reply('logs_download', { kind: 'downloaded', entry: { id: 0, size: 360, time_utc: 1700000000 }, path: '/tmp/log0.ulg' }))
  await wait(async () => !(await state()).busy)
  await page.getByText('Saved ✓', { exact: true }).waitFor()
  check(await page.getByText('Saved ✓', { exact: true }).isVisible(), 'only durable typed reply marks Saved')
  await page.screenshot({ path: join(shots, 'durable-download.png') })
  await list()
  await page.getByRole('button', { name: 'Download', exact: true }).click()
  await wait(() => pending('logs_download'))
  const cancelled = await pending('logs_download')
  await page.getByRole('button', { name: 'Cancel', exact: true }).click()
  await wait(async () => !(await state()).busy)
  const cancellation = (await calls()).findLast((call) => call.command === 'logs_cancel')
  check(cancellation.args.operation.id === cancelled.operation.id && cancellation.args.session.owner.instance_id === cancelled.context.session.owner.instance_id, 'cancel addresses the exact operation/session')
  await list()
  accept = false; dialogs.length = 0
  const count = (await calls()).length
  await page.getByRole('button', { name: 'Delete', exact: true }).click()
  check(dialogs.length === 1 && (await calls()).length === count, 'declining first deletion confirmation writes nothing')
  accept = true; dialogs.length = 0
  await page.getByRole('button', { name: 'Delete', exact: true }).click()
  await wait(() => pending('logs_delete'))
  const deletion = await pending('logs_delete')
  check(dialogs.length === 2 && deletion.input.confirmation === 'DELETE LOG 0' && deletion.input.entry.size === 360, 'two confirmations carry identity-bound native keyword')
  await page.evaluate(() => window.__reply('logs_delete', { kind: 'deleted', entries: [] }))
  await wait(async () => !(await state()).busy)
  await list()
  dialogs.length = 0
  await page.getByRole('button', { name: 'Delete all…', exact: true }).click()
  await wait(() => pending('logs_erase_all'))
  check(dialogs.length === 2 && (await pending('logs_erase_all')).input.confirmation === 'DELETE ALL LOGS', 'erase-all requires both confirmations')
  await page.evaluate(() => window.__fail('logs_erase_all', { code: 'reconciliation_required', reason: 'Deletion dispatched; directory read-back is unconfirmed' }))
  await wait(async () => !(await state()).busy)
  check((await state()).reconciliation && (await state()).entries.length === 0, 'post-delete uncertainty discards old list and blocks destruction')
  await page.screenshot({ path: join(shots, 'deletion-uncertain.png') })
  await list([])
  check((await state()).entries.length === 0 && !(await state()).reconciliation, 'fresh explicit empty list reconciles UI state')
  await page.screenshot({ path: join(shots, 'empty-list.png') })
  await page.getByRole('button', { name: 'Refresh list', exact: true }).click()
  await wait(() => pending('logs_request_list'))
  const old = await pending('logs_request_list')
  await page.evaluate(() => window.__replace())
  await page.getByRole('button', { name: 'Refresh list', exact: true }).click()
  await wait(async () => (await page.evaluate(() => window.__pending().length)) === 2)
  await page.evaluate(() => window.__reply('logs_request_list', { kind: 'listed', entries: [{ id: 99, size: 1, time_utc: 1 }] }))
  check((await state()).busy && (await state()).entries.length === 0, 'late old-target list cannot complete replacement operation')
  await page.evaluate((entry) => window.__reply('logs_request_list', { kind: 'listed', entries: [entry] }), entry)
  await wait(async () => !(await state()).busy)
  await page.evaluate((old) => window.__emit('logs_progress', { operation: old.operation, request_id: old.context.request_id, entry: { id: 0, size: 360, time_utc: 1700000000 }, fetched: 360, total: 360 }), old)
  check(await page.evaluate(() => window.__logs.getState().downloads.size === 0), 'old progress cannot mark replacement log saved')
  await page.getByRole('button', { name: 'ver', exact: true }).click()
  await wait(async () => await page.evaluate(() => window.__console.getState().transcript.includes('shell response')))
  check(await page.getByRole('button', { name: 'Refresh list', exact: true }).isDisabled(), 'active console excludes log transfer UI')
  const shell = await page.evaluate(() => window.__console.getState().operation)
  await page.evaluate((shell) => window.__emit('console_output', { operation: shell, target: shell.target, sequence: '3', received_at_ms: Date.now(), chunk: 'gap\n' }), shell)
  check(await page.evaluate(() => window.__console.getState().outputGap), 'missing shell sequence is visible')
  await page.getByTestId('console-command').fill('listener sensor_combined; reboot')
  await page.getByTestId('console-command').press('Enter')
  await page.getByTestId('console-confirm').waitFor()
  check(await page.getByTestId('console-confirm-run').isDisabled(), 'compound read-only prefix still requires exact RUN')
  await page.locator('[data-testid="console-confirm"] input').fill('RUN')
  await page.screenshot({ path: join(shots, 'shell-confirm.png') })
  await page.getByTestId('console-confirm-run').click()
  await wait(async () => (await calls()).filter((call) => call.command === 'console_send').length === 2)
  check((await calls()).findLast((call) => call.command === 'console_send').args.request.confirmation === 'RUN', 'free-form command carries native confirmation')
  await page.evaluate((shell) => window.__emit('console_output', { operation: shell, target: shell.target, sequence: '5', received_at_ms: Date.now(), chunk: 'history\n'.repeat(1000) }), shell)
  const output = page.getByTestId('console-output')
  await output.evaluate((element) => { element.scrollTop = 0; element.dispatchEvent(new Event('scroll')) })
  await page.evaluate((shell) => window.__emit('console_output', { operation: shell, target: shell.target, sequence: '6', received_at_ms: Date.now(), chunk: 'new\n' }), shell)
  check(await output.evaluate((element) => element.scrollTop < 10), 'new shell output preserves history scroll position')
  await page.evaluate((shell) => window.__emit('console_output', { operation: shell, target: shell.target, sequence: '7', received_at_ms: Date.now(), chunk: 'x'.repeat(70000) }), shell)
  check(await page.evaluate(() => window.__console.getState().transcript.length === 65536), 'shell transcript stays within 64 KiB character budget')
  await page.screenshot({ path: join(shots, 'shell-bounded-output.png') })
  await page.getByTestId('console-close').click()
  await wait(async () => !await page.evaluate(() => window.__console.getState().operation))
  check((await calls()).findLast((call) => call.command === 'console_close').args.operation.id === shell.id, 'console close uses its original operation')
  await page.evaluate((shell) => window.__emit('console_output', { operation: shell, target: shell.target, sequence: '8', received_at_ms: Date.now(), chunk: 'OLD-CLOSED-SESSION' }), shell)
  check(!await page.evaluate(() => window.__console.getState().transcript.includes('OLD-CLOSED-SESSION')), 'closed console output cannot revive a retired session')
  const grip = await page.getByTestId('console-grip').boundingBox()
  await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2)
  await page.mouse.down()
  await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2 - 40)
  await page.mouse.up()
  check(await page.evaluate(() => Number(localStorage.getItem('maggcs.consoleHeight')) === 200), 'resize persists final pointer height')
  await page.getByTestId('console-command').fill('reboot')
  await page.getByTestId('console-command').press('Enter')
  await page.getByTestId('console-confirm').waitFor()
  await page.locator('[data-testid="console-confirm"] input').fill('RUN')
  const sendsBeforeReplacement = (await calls()).filter((call) => call.command === 'console_send').length
  await page.evaluate(() => window.__stealthReplace())
  await page.getByTestId('console-confirm-run').click()
  await page.waitForTimeout(100)
  check((await calls()).filter((call) => call.command === 'console_send').length === sendsBeforeReplacement, 'reviewed free-form command cannot follow a stealth target replacement')
  await page.evaluate(() => window.__replace())
  await list()
  await page.getByTestId('right-dock-tab-drape').click()
  await page.getByTestId('drape-panel').waitFor()
  check(await page.getByTestId('right-dock-tab-drape').getAttribute('aria-selected') === 'true', 'Drape tab shows the drape monitor')
  await page.getByTestId('right-dock-tab-logs').click()
  await page.getByTestId('right-dock-logs').waitFor()
  check(await page.getByTestId('right-dock-tab-logs').getAttribute('aria-selected') === 'true', 'Logs tab returns to logs and console')
  const dockBefore = await page.getByTestId('right-dock').boundingBox()
  const dockGrip = await page.getByTestId('right-dock-resize').boundingBox()
  await page.mouse.move(dockGrip.x + dockGrip.width / 2, dockGrip.y + 40)
  await page.mouse.down()
  await page.mouse.move(dockGrip.x + dockGrip.width / 2 - 40, dockGrip.y + 40)
  await page.mouse.up()
  const dockWide = await page.getByTestId('right-dock').boundingBox()
  check(dockWide.width > dockBefore.width + 20, 'dragging the divider widens the right dock')
  check(await page.evaluate(() => Number(localStorage.getItem('maggcs.rightWidth')) === Math.round(document.querySelector('[data-testid=right-dock]').getBoundingClientRect().width)), 'right dock width persists')
  await page.mouse.move(dockWide.x + 0.5, dockWide.y + 40)
  await page.mouse.down()
  await page.mouse.move(dockWide.x + 0.5 + 40, dockWide.y + 40)
  await page.mouse.up()
  await page.screenshot({ path: join(shots, 'right-dock.png') })
  await page.setViewportSize({ width: 430, height: 900 })
  await page.screenshot({ path: join(shots, 'responsive-logs.png') })
  check(await page.getByTestId('right-dock').evaluate((element) => element.scrollWidth <= element.clientWidth + 1), 'responsive right dock has no horizontal overflow')
  await page.evaluate(() => document.documentElement.classList.add('field-mode'))
  await page.screenshot({ path: join(shots, 'field-mode-logs.png') })
  check(await page.getByTestId('right-dock').evaluate((element) => element.scrollWidth <= element.clientWidth + 1), 'field-mode right dock remains within the viewport')
  await page.evaluate(() => document.documentElement.classList.remove('field-mode'))
  await page.evaluate(() => window.__emit('link', { link_state: 'disconnected', endpoint: 'fixture', fc_alive: false }))
  await page.screenshot({ path: join(shots, 'disconnected-logs.png') })
  check(await page.getByRole('button', { name: 'Refresh list', exact: true }).isDisabled(), 'disconnected log actions are disabled')
  check((await state()).entries.length === 0, 'disconnect retires target-scoped list')
  await page.getByTestId('right-dock-close').click()
  await page.getByTestId('right-dock').waitFor({ state: 'detached' })
  await page.evaluate(() => window.__emit('link', { link_state: 'connected', endpoint: 'fixture', fc_alive: true }))
  await page.getByTestId('tools-open').click()
  await page.getByTestId('tool-logs').click()
  await page.getByTestId('right-dock').waitFor()
  check(await page.getByTestId('right-dock-logs').isVisible(), 'Logs reopens in the right dock')
  check(await page.getByTestId('console-output').evaluate((element) => Math.round(element.getBoundingClientRect().height) === 200), 'dock reopen restores final console height')
  await page.screenshot({ path: join(shots, 'reopened-logs.png') })
  check(errors.length === 0, `no browser exceptions: ${errors.join('; ')}`)
} finally { await browser.close() }
if (failed) process.exitCode = 1
