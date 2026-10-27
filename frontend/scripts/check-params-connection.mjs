// Real React Parameters UI with a Tauri IPC stub matching the Rust wire types.
// Covers cached firmware replies, delayed tagged values with synchronized editor
// drafts, full current/default display, search virtualization and reconnects.
// PLAYWRIGHT_MODULE=/path/to/playwright node scripts/check-params-connection.mjs [url] [screenshots]
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { readFile, mkdir } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { join } from 'node:path'

const require = createRequire(import.meta.url)
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright')
const url = process.argv[2] ?? 'http://127.0.0.1:1420/params.html'
const shots = process.argv[3]
const WAIT_TIMEOUT_MS = 6000
const WAIT_POLL_MS = 50
const bytes = await readFile(new URL('../public/param-meta/parameters.json', import.meta.url))
const catalog = JSON.parse(bytes)
const manifest = JSON.parse(await readFile(new URL('../public/param-meta/manifest.json', import.meta.url)))
assert.equal(catalog.length, manifest.parameterCount)
assert.equal(new Set(catalog.map((entry) => entry.name)).size, catalog.length)
assert.equal(createHash('sha256').update(bytes).digest('hex'), manifest.catalogSha256)
assert.equal(manifest.firmwareVersion, '1.17.0')
assert.equal(manifest.target, 'px4_fmu-v6x_default')
assert.ok(catalog.some((entry) => entry.name === 'USB_MAV_MODE' && entry.type === 'Int32'))

const browser = await chromium.launch({ headless: true })
const errors = []
if (shots) await mkdir(shots, { recursive: true })

async function openPage(initiallyConnected) {
  const context = await browser.newContext({ viewport: { width: 1366, height: 768 } })
  const page = await context.newPage()
  page.on('pageerror', (error) => errors.push(String(error)))
  await page.addInitScript((alive) => {
    const callbacks = new Map()
    const listeners = new Map()
    let callbackId = 0
    let listenerId = 0
    const bench = {
      calls: [],
      generation: 1,
      target() {
        return { vehicle_id: 'vehicle-a', session_id: `session-${this.generation}`, link_id: '1',
          generation: String(this.generation), mavlink_target: { system_id: 42, component_id: 1 } }
      },
      link: { link_state: 'connected', endpoint: 'serial:/dev/ttyACM0:115200', fc_alive: alive },
      version: '1.17.0',
      nullVersions: 0,
      deferVersion: false,
      deferRead: null,
      readDelayMs: 0,
      delayedReads: [],
      pendingReads: [],
      pendingVersions: [],
      entries: {
        USB_MAV_MODE: { name: 'USB_MAV_MODE', index: 1, ptype: 'i32', value: { int: 0 } },
        MPC_XY_VEL_MAX: { name: 'MPC_XY_VEL_MAX', index: 2, ptype: 'real32', value: { real: 12.5 } },
        COM_RC_IN_MODE: { name: 'COM_RC_IN_MODE', index: 3, ptype: 'i32', value: { int: 0 } },
        FUTURE_PARAM: { name: 'FUTURE_PARAM', index: 4, ptype: 'i32', value: { int: 3 } },
        RTL_TYPE: { name: 'RTL_TYPE', index: 5, ptype: 'i32', value: { int: 1 } },
      },
      emit(event, payload) {
        if (event === 'params_event') payload = { target: this.target(), operation_id: null, ...payload }
        for (const [id, listener] of listeners) {
          if (listener.event === event) callbacks.get(listener.handler)?.({ id, event, payload })
        }
      },
      setLink(connected) {
        if (connected) this.generation += 1
        this.link = { ...this.link, fc_alive: connected }
        this.emit('link', structuredClone(this.link))
      },
    }
    window.__paramsBench = bench
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (_event, id) => listeners.delete(id),
    }
    window.__TAURI_INTERNALS__ = {
      transformCallback(callback) { callbacks.set(++callbackId, callback); return callbackId },
      unregisterCallback(id) { callbacks.delete(id) },
      async invoke(command, args) {
        bench.calls.push({ command, args })
        if (command === 'plugin:event|listen') {
          listeners.set(++listenerId, args)
          return listenerId
        }
        if (command === 'plugin:event|unlisten') { listeners.delete(args.eventId); return }
        if (command === 'config_load') return null
        if (command === 'alert_snapshot') return []
        if (command === 'link_status') return structuredClone(bench.link)
        if (command === 'runtime_targets') return bench.link.fc_alive ? [{ link_id: '1', target: bench.target(), availability: 'available',
          heartbeat_age_ms: 0, parameter_service_available: true, roles: ['primary'], transport_owner: 'primary', endpoint: bench.link.endpoint }] : []
        if (command === 'runtime_client_attach') return { owner: { client_id: 'client-params', instance_id: `instance-${bench.generation}` }, target: structuredClone(args.target), module_id: args.moduleId }
        if (command === 'runtime_client_validate') {
          if (args.session.target.generation !== String(bench.generation)) throw { code: 'target_changed' }
          return null
        }
        if (command === 'runtime_client_detach') return null
        if (command === 'params_fc_version') {
          if (bench.deferVersion) {
            bench.deferVersion = false
            return new Promise((resolve) => bench.pendingVersions.push(resolve))
          }
          if (bench.nullVersions > 0) { bench.nullVersions -= 1; return null }
          return bench.version // Rust Option<String> is a scalar, not {fcVersion}.
        }
        if (command === 'params_read') {
          const { name, context } = args.request
          assertContext(context)
          const entry = structuredClone(bench.entries[name] ?? null)
          if (bench.deferRead === name) {
            bench.deferRead = null
            return new Promise((resolve) => bench.pendingReads.push(() => resolve(entry)))
          }
          if (bench.readDelayMs > 0) {
            const delayMs = bench.readDelayMs
            bench.delayedReads.push({ name, delayMs })
            await new Promise((resolve) => setTimeout(resolve, delayMs))
          }
          return entry
        }
        if (command === 'params_cancel') return null
        return null
      },
    }
    function assertContext(context) {
      if (!context?.request_id || context.session?.target?.generation !== String(bench.generation)) {
        throw { code: 'target_changed' }
      }
    }
  }, initiallyConnected)
  // Readiness is the rendered table. A live dev server can retain background
  // traffic, so global network idleness is not a page acceptance criterion.
  await page.goto(url, { waitUntil: 'domcontentloaded' })
  await page.locator('.p-row').first().waitFor()
  return { page, context }
}

async function until(page, check, label) {
  const deadline = Date.now() + WAIT_TIMEOUT_MS
  while (!(await check())) {
    assert.ok(Date.now() < deadline, label)
    await page.waitForTimeout(WAIT_POLL_MS)
  }
}

const badge = (page) => page.locator('.p-toolbar .p-fc').innerText()
const rowValue = (page) => page.locator('.p-row.sel .p-value').innerText()
// Observe both render and draft in one browser task: the editor effect can run
// after the row already displays the new FC value.
async function waitForSelectedValueAndDraft(page, {
  row, draft, label, editor = '[aria-label="New value"]', name,
}) {
  const pattern = row instanceof RegExp ? { source: row.source, flags: row.flags } : null
  await until(page, () => page.evaluate(({ row, pattern, draft, editor, name }) => {
    const selected = document.querySelector('.p-row.sel')
    const cell = selected?.querySelector('.p-value')
    const input = document.querySelector(editor)
    if (!cell || !input || input.value !== draft) return false
    if (name && document.querySelector('.p-detail-title')?.textContent.trim() !== name) return false
    const displayed = cell.innerText.trim()
    return pattern ? new RegExp(pattern.source, pattern.flags).test(displayed) : displayed === row
  }, { row: pattern ? null : row, pattern, draft, editor, name }), label)
}

const search = async (page, name) => {
  await page.locator('.p-search').fill(name)
  await until(page, () => page.evaluate((name) => {
    const cell = document.querySelector('.p-row .p-name')
    const displayed = cell && [...cell.childNodes]
      .filter((node) => node.nodeType === Node.TEXT_NODE)
      .map((node) => node.textContent).join('').trim()
    return displayed === name.toUpperCase()
  }, name), `${name} is visible`)
  await page.locator('.p-row').first().click()
}
const capture = async (page, name) => {
  if (shots) await page.screenshot({ path: join(shots, `${name}.png`) })
}

async function assertValueSummary(page, current, defaultValue, label) {
  const summary = await page.$$eval('.p-detail-values > div', (rows) => rows.map((row) => {
    const value = row.querySelector('dd')
    const cell = value.getBoundingClientRect()
    const pane = row.closest('.p-detail').getBoundingClientRect()
    const range = document.createRange()
    range.selectNodeContents(value)
    return {
      label: row.querySelector('dt').textContent,
      text: value.textContent.trim(),
      visible: cell.top >= pane.top && cell.bottom <= pane.bottom && cell.bottom <= window.innerHeight,
      fullTextFits: [...range.getClientRects()].every((line) =>
        line.left >= cell.left - 1 && line.right <= cell.right + 1
        && line.top >= cell.top - 1 && line.bottom <= cell.bottom + 1),
    }
  }))
  assert.deepEqual(summary.map((value) => value.label), ['Current value', 'Default'], `${label}: separate value labels`)
  assert.deepEqual(summary.map((value) => value.text), [current, defaultValue], `${label}: FC value and metadata default`)
  for (const value of summary) {
    assert.ok(value.visible, `${label}: ${value.label} is fully visible above the description`)
    assert.ok(value.fullTextFits, `${label}: ${value.label} wraps without truncation`)
  }
}

try {
  const { page, context } = await openPage(true)
  await until(page, async () => await badge(page) === 'PX4 1.17.0', 'cached scalar firmware reply updates badge')

  // Search after deep scrolling must show a short result set, not an empty
  // virtualized slice starting thousands of pixels below its only row.
  await page.locator('.p-scroll').evaluate((el) => { el.scrollTop = 4000 })
  await page.waitForTimeout(80)
  await search(page, 'usb_mav_mode')
  await waitForSelectedValueAndDraft(page, {
    name: 'USB_MAV_MODE', row: '0 · normal', draft: '0', editor: '.p-edit-select',
    label: 'tagged integer zero and numeric draft are synchronized',
  })
  assert.equal(await page.locator('.p-scroll').evaluate((el) => el.scrollTop), 0)
  assert.equal(await page.locator('.p-hitcount').innerText(), `1/${manifest.parameterCount}`)
  assert.equal(await page.locator('.p-row.sel .p-default').innerText(), '2 · onboard', 'default has its own table cell')
  await assertValueSummary(page, '0 · normal', '2 · onboard', 'USB_MAV_MODE')
  await capture(page, 'params-live-usb')

  await page.evaluate(() => window.__paramsBench.emit('params_event', {
    kind: 'external_change', name: 'USB_MAV_MODE', ptype: 'i32', value: { int: 5 },
  }))
  await waitForSelectedValueAndDraft(page, {
    name: 'USB_MAV_MODE', row: '5 · config', draft: '5', editor: '.p-edit-select',
    label: 'external change synchronizes the row and enum draft',
  })

  await page.evaluate(() => { window.__paramsBench.readDelayMs = 250 })
  await search(page, 'MPC_XY_VEL_MAX')
  await waitForSelectedValueAndDraft(page, {
    name: 'MPC_XY_VEL_MAX', row: /^12\.5(?:\s|$)/, draft: '12.5',
    label: 'delayed float read synchronizes the row and numeric draft',
  })
  assert.ok(await page.evaluate(() => window.__paramsBench.delayedReads.some((read) =>
    read.name === 'MPC_XY_VEL_MAX' && read.delayMs === 250)), 'float fixture actually delays its IPC reply')
  await capture(page, 'params-delayed-float')
  await page.evaluate(() => { window.__paramsBench.readDelayMs = 0 })

  // Long enum meanings and firmware descriptions must not hide either value
  // below the fold, including at the window and editor's minimum widths.
  const rtl = catalog.find((entry) => entry.name === 'RTL_TYPE')
  const rtlCurrent = `1 · ${rtl.values.find((option) => option.value === 1).description}`
  const rtlDefault = `0 · ${rtl.values.find((option) => option.value === 0).description}`
  assert.ok(rtlCurrent.length > 200 && rtlDefault.length > 60, 'both test values have long enum meanings')
  await page.setViewportSize({ width: 900, height: 600 })
  await search(page, 'RTL_TYPE')
  await waitForSelectedValueAndDraft(page, {
    name: 'RTL_TYPE', row: rtlCurrent, draft: '1', editor: '.p-edit-select',
    label: 'long enum row and draft are synchronized',
  })
  await assertValueSummary(page, rtlCurrent, rtlDefault, '900x600 long values')
  await capture(page, 'params-long-values-900')
  await page.locator('.p-split.right').focus()
  for (let i = 0; i < 12; i++) await page.keyboard.press('ArrowRight')
  await assertValueSummary(page, rtlCurrent, rtlDefault, '140 px editor long values')
  await capture(page, 'params-long-values-narrow')
  await page.locator('.p-detail').evaluate((el) => { el.scrollTop = el.scrollHeight })
  assert.ok(await page.locator('.p-detail').evaluate((el) => el.scrollTop > 0), 'long parameter details can be scrolled')
  await page.locator('.p-split.right').dblclick()
  await page.setViewportSize({ width: 1366, height: 768 })

  // Leave an old read unresolved, recover on the same endpoint, and check
  // that the retained selection automatically starts a new read.
  await page.evaluate(() => { window.__paramsBench.deferRead = 'COM_RC_IN_MODE' })
  await search(page, 'COM_RC_IN_MODE')
  await until(page, () => page.evaluate(() => window.__paramsBench.pendingReads.length === 1), 'old read is pending')
  assert.equal(await page.locator('.p-detail').evaluate((el) => el.scrollTop), 0, 'selecting another row returns to the value summary')
  await page.evaluate(() => window.__paramsBench.setLink(false))
  await until(page, async () => await badge(page) === 'FC disconnected', 'loss is shown in the parameters window')
  assert.equal(await page.getByRole('button', { name: 'Pull all', exact: true }).isDisabled(), true)
  assert.equal(await page.evaluate(() => window.__mgParams.getState().values.size), 0, 'old values cleared')
  await capture(page, 'params-disconnected')
  await page.evaluate(() => {
    window.__paramsBench.entries.COM_RC_IN_MODE.value = { int: 1 }
    window.__paramsBench.readDelayMs = 150
    window.__paramsBench.setLink(true)
  })
  await until(page, async () => await badge(page) === 'PX4 1.17.0', 'reconnect refreshes version')
  await waitForSelectedValueAndDraft(page, {
    name: 'COM_RC_IN_MODE', row: /^1(?:\s|$)/, draft: '1', editor: '.p-edit-select',
    label: 'delayed reconnect read synchronizes the retained selection and draft',
  })
  assert.ok(await page.evaluate(() => window.__paramsBench.delayedReads.some((read) =>
    read.name === 'COM_RC_IN_MODE' && read.delayMs === 150)), 'reconnect fixture actually delays its new read')
  await page.evaluate(() => window.__paramsBench.pendingReads.shift()())
  await page.waitForTimeout(150)
  assert.ok((await rowValue(page)).startsWith('1'), 'old read cannot overwrite the recovered value')
  assert.equal(await page.locator('.p-edit-select').inputValue(), '1', 'old reply cannot overwrite the recovered draft')
  await capture(page, 'params-reconnected')

  // A delayed old version reply must also stay scoped to its link epoch.
  await page.evaluate(() => {
    window.__paramsBench.deferVersion = true
    window.__paramsBench.setLink(false)
    window.__paramsBench.setLink(true)
  })
  await until(page, () => page.evaluate(() => window.__paramsBench.pendingVersions.length === 1), 'old version request is pending')
  await page.evaluate(() => {
    window.__paramsBench.setLink(false)
    window.__paramsBench.version = '1.17.1'
    window.__paramsBench.setLink(true)
  })
  await until(page, async () => await badge(page) === 'PX4 1.17.1', 'second reconnect accepts new version')
  await page.evaluate(() => window.__paramsBench.pendingVersions.shift()('1.16.0'))
  await page.waitForTimeout(150)
  assert.equal(await badge(page), 'PX4 1.17.1', 'old firmware reply discarded')
  assert.ok((await page.locator('.p-version-note').innerText()).includes('1.17.0'), 'manifest drives version mismatch notice')

  // A firmware-only parameter can be read by exact name even without metadata.
  await page.locator('.p-search').fill('FUTURE_PARAM')
  await page.getByRole('button', { name: 'Read FUTURE_PARAM from FC', exact: true }).click()
  await waitForSelectedValueAndDraft(page, {
    name: 'FUTURE_PARAM', row: '3', draft: '3',
    label: 'firmware-only parameter synchronizes its new row and draft',
  })
  await assertValueSummary(page, '3', '—', 'parameter without metadata')
  const writeCalls = await page.evaluate(() => window.__paramsBench.calls.filter((call) => ['params_set', 'params_reboot'].includes(call.command)))
  assert.deepEqual(writeCalls, [], 'connection checks do not write parameter values')
  await context.close()

  const late = await openPage(false)
  await until(late.page, async () => await badge(late.page) === 'No FC', 'window can open without a live link')
  await search(late.page, 'USB_MAV_MODE')
  assert.equal(await late.page.evaluate(() => window.__paramsBench.calls.filter((call) => call.command === 'params_read').length), 0)
  await late.page.evaluate(() => {
    window.__paramsBench.nullVersions = 1 // Runtime attaches slightly after the first heartbeat.
    window.__paramsBench.readDelayMs = 120
    window.__paramsBench.setLink(true)
  })
  await until(late.page, async () => await badge(late.page) === 'PX4 1.17.0', 'late connection recovers firmware without a window reload')
  await waitForSelectedValueAndDraft(late.page, {
    name: 'USB_MAV_MODE', row: '0 · normal', draft: '0', editor: '.p-edit-select',
    label: 'late connection synchronizes the retained row and editor',
  })
  assert.ok(await late.page.evaluate(() => window.__paramsBench.delayedReads.some((read) =>
    read.name === 'USB_MAV_MODE' && read.delayMs === 120)), 'late-connection fixture exercises a delayed parameter reply')
  assert.ok(await late.page.evaluate(() => window.__paramsBench.calls.filter((call) => call.command === 'params_fc_version').length >= 2), 'unanswered version query retried')
  await capture(late.page, 'params-late-connect')
  await late.context.close()

  assert.deepEqual(errors, [])
  console.log(`PASS: ${catalog.length}-parameter catalog; synchronized row/draft under delayed reads, current/default values, search, external updates, reconnects and late replies`)
} finally {
  await browser.close()
}
