// Real React Parameters UI with a Tauri IPC stub matching the Rust wire types.
// Covers cached firmware replies, tagged values, search virtualization, repeated
// reconnects, late old replies and a window opened before the FC connects.
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
      link: { link_state: 'connected', endpoint: 'serial:/dev/ttyACM0:115200', fc_alive: alive },
      version: '1.17.0',
      nullVersions: 0,
      deferVersion: false,
      deferRead: null,
      pendingReads: [],
      pendingVersions: [],
      entries: {
        USB_MAV_MODE: { name: 'USB_MAV_MODE', index: 1, ptype: 'i32', value: { int: 0 } },
        MPC_XY_VEL_MAX: { name: 'MPC_XY_VEL_MAX', index: 2, ptype: 'real32', value: { real: 12.5 } },
        COM_RC_IN_MODE: { name: 'COM_RC_IN_MODE', index: 3, ptype: 'i32', value: { int: 0 } },
        FUTURE_PARAM: { name: 'FUTURE_PARAM', index: 4, ptype: 'i32', value: { int: 3 } },
      },
      emit(event, payload) {
        for (const [id, listener] of listeners) {
          if (listener.event === event) callbacks.get(listener.handler)?.({ id, event, payload })
        }
      },
      setLink(connected) {
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
        if (command === 'params_fc_version') {
          if (bench.deferVersion) {
            bench.deferVersion = false
            return new Promise((resolve) => bench.pendingVersions.push(resolve))
          }
          if (bench.nullVersions > 0) { bench.nullVersions -= 1; return null }
          return bench.version // Rust Option<String> is a scalar, not {fcVersion}.
        }
        if (command === 'params_read') {
          const entry = structuredClone(bench.entries[args.name] ?? null)
          if (bench.deferRead === args.name) {
            bench.deferRead = null
            return new Promise((resolve) => bench.pendingReads.push(() => resolve(entry)))
          }
          return entry
        }
        if (command === 'params_cancel') return null
        return null
      },
    }
  }, initiallyConnected)
  // Readiness is the rendered table. A live dev server can retain background
  // traffic, so global network idleness is not a page acceptance criterion.
  await page.goto(url, { waitUntil: 'domcontentloaded' })
  await page.locator('.p-row').first().waitFor()
  return { page, context }
}

async function until(page, check, label) {
  const deadline = Date.now() + 6000
  while (!(await check())) {
    assert.ok(Date.now() < deadline, label)
    await page.waitForTimeout(50)
  }
}

const badge = (page) => page.locator('.p-toolbar .p-fc').innerText()
const rowValue = (page) => page.locator('.p-row.sel .p-value').innerText()
const search = async (page, name) => {
  await page.locator('.p-search').fill(name)
  await until(page, async () => await page.locator('.p-row').count() > 0, `${name} is visible`)
  await page.locator('.p-row').first().click()
}
const capture = async (page, name) => {
  if (shots) await page.screenshot({ path: join(shots, `${name}.png`) })
}

try {
  const { page, context } = await openPage(true)
  await until(page, async () => await badge(page) === 'PX4 1.17.0', 'cached scalar firmware reply updates badge')

  // Search after deep scrolling must show a short result set, not an empty
  // virtualized slice starting thousands of pixels below its only row.
  await page.locator('.p-scroll').evaluate((el) => { el.scrollTop = 4000 })
  await page.waitForTimeout(80)
  await search(page, 'usb_mav_mode')
  await until(page, async () => await rowValue(page) === '0 · normal', 'tagged integer zero is rendered with enum meaning')
  assert.equal(await page.locator('.p-edit-select').inputValue(), '0', 'draft is numeric, not JSON object text')
  assert.equal(await page.locator('.p-scroll').evaluate((el) => el.scrollTop), 0)
  assert.equal(await page.locator('.p-hitcount').innerText(), `1/${manifest.parameterCount}`)
  await capture(page, 'params-live-usb')

  await page.evaluate(() => window.__paramsBench.emit('params_event', {
    kind: 'external_change', name: 'USB_MAV_MODE', ptype: 'i32', value: { int: 5 },
  }))
  await until(page, async () => await rowValue(page) === '5 · config', 'external changes replace the old value')
  assert.equal(await page.locator('.p-edit-select').inputValue(), '5')

  await search(page, 'MPC_XY_VEL_MAX')
  await until(page, async () => (await rowValue(page)).startsWith('12.5'), 'tagged float is rendered')
  assert.equal(await page.getByLabel('New value').inputValue(), '12.5')

  // Leave an old read unresolved, recover on the same endpoint, and check
  // that the retained selection automatically starts a new read.
  await page.evaluate(() => { window.__paramsBench.deferRead = 'COM_RC_IN_MODE' })
  await search(page, 'COM_RC_IN_MODE')
  await until(page, () => page.evaluate(() => window.__paramsBench.pendingReads.length === 1), 'old read is pending')
  await page.evaluate(() => window.__paramsBench.setLink(false))
  await until(page, async () => await badge(page) === 'FC disconnected', 'loss is shown in the parameters window')
  assert.equal(await page.getByRole('button', { name: 'Pull all', exact: true }).isDisabled(), true)
  assert.equal(await page.evaluate(() => window.__mgParams.getState().values.size), 0, 'old values cleared')
  await capture(page, 'params-disconnected')
  await page.evaluate(() => {
    window.__paramsBench.entries.COM_RC_IN_MODE.value = { int: 1 }
    window.__paramsBench.setLink(true)
  })
  await until(page, async () => await badge(page) === 'PX4 1.17.0' && (await rowValue(page)).startsWith('1'), 'reconnect refreshes version and selected value')
  await page.evaluate(() => window.__paramsBench.pendingReads.shift()())
  await page.waitForTimeout(150)
  assert.ok((await rowValue(page)).startsWith('1'), 'old read cannot overwrite the recovered value')
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
  await until(page, async () => await page.locator('.p-row.sel').count() > 0 && await rowValue(page) === '3', 'unknown FC parameter is added to the table')
  assert.equal(await page.getByLabel('New value').inputValue(), '3')
  const writeCalls = await page.evaluate(() => window.__paramsBench.calls.filter((call) => ['params_set', 'params_reboot'].includes(call.command)))
  assert.deepEqual(writeCalls, [], 'connection checks do not write parameter values')
  await context.close()

  const late = await openPage(false)
  await until(late.page, async () => await badge(late.page) === 'No FC', 'window can open without a live link')
  await search(late.page, 'USB_MAV_MODE')
  assert.equal(await late.page.evaluate(() => window.__paramsBench.calls.filter((call) => call.command === 'params_read').length), 0)
  await late.page.evaluate(() => {
    window.__paramsBench.nullVersions = 1 // Runtime attaches slightly after the first heartbeat.
    window.__paramsBench.setLink(true)
  })
  await until(late.page, async () => await badge(late.page) === 'PX4 1.17.0' && await rowValue(late.page) === '0 · normal', 'late connection recovers without a window reload')
  assert.ok(await late.page.evaluate(() => window.__paramsBench.calls.filter((call) => call.command === 'params_fc_version').length >= 2), 'unanswered version query retried')
  await capture(late.page, 'params-late-connect')
  await late.context.close()

  assert.deepEqual(errors, [])
  console.log(`PASS: ${catalog.length}-parameter catalog; scalar firmware, tagged values, search, external updates, reconnects and late replies`)
} finally {
  await browser.close()
}
