// Real Inspector UI with an isolated IPC stub: a retained tap/transport must
// not keep the source chip saying Live link after the primary FC is lost.
// PLAYWRIGHT_MODULE=/path/to/playwright node scripts/check-inspector-connection.mjs [url] [screenshots]
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
const require = createRequire(import.meta.url)
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright')
const url = process.argv[2] ?? 'http://127.0.0.1:1420/inspector.html'
const shots = process.argv[3]
const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ viewport: { width: 1366, height: 768 } })
const errors = []
page.on('pageerror', (error) => errors.push(String(error)))
await page.addInitScript(() => {
  const callbacks = new Map()
  let id = 0
  window.__inspectorBench = {
    status: { messages: 100, samples: 100, dropped: 0, connected: true, tap_running: true, replay_active: false },
    replay: { active: false, name: null, duration_ms: 1000, position_ms: 0, playing: false, speed: 1 },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} }
  window.__TAURI_INTERNALS__ = {
    transformCallback(callback) { callbacks.set(++id, callback); return id },
    unregisterCallback(key) { callbacks.delete(key) },
    async invoke(command, args) {
      if (command === 'plugin:event|listen') return args.handler
      if (command === 'inspector_status') return structuredClone(window.__inspectorBench.status)
      if (command === 'inspector_replay_status') return structuredClone(window.__inspectorBench.replay)
      if (['inspector_catalog', 'inspector_list_algorithms', 'alert_snapshot'].includes(command)) return []
      if (command === 'inspector_source') return 'live'
      return null
    },
  }
})
async function waitChip(label) {
  const deadline = Date.now() + 5000
  while ((await page.locator('.tool-header .source-chip').innerText()) !== label) {
    assert.ok(Date.now() < deadline, `source chip becomes ${label}`)
    await page.waitForTimeout(50)
  }
}
try {
  await page.goto(url, { waitUntil: 'domcontentloaded' })
  await page.locator('.inspector-grid').waitFor()
  await waitChip('Live link')
  await page.evaluate(() => { window.__inspectorBench.status.connected = false })
  await waitChip('FC disconnected')
  assert.ok((await page.locator('.tool-status').innerText()).includes('0 msg/s'))
  if (shots) {
    await mkdir(shots, { recursive: true })
    await page.screenshot({ path: join(shots, 'inspector-disconnected.png') })
  }
  await page.evaluate(() => {
    window.__inspectorBench.status.connected = true
    window.__inspectorBench.status.messages += 20
  })
  await waitChip('Live link')
  if (shots) await page.screenshot({ path: join(shots, 'inspector-reconnected.png') })
  await page.evaluate(() => {
    window.__inspectorBench.status.connected = false
    window.__inspectorBench.status.replay_active = true
    window.__inspectorBench.replay.active = true
    window.__inspectorBench.replay.name = 'test.ulg'
  })
  await waitChip('Replay · test.ulg')
  assert.deepEqual(errors, [])
  console.log('PASS: Inspector live → disconnected → restored; replay remains an independent source')
} finally {
  await browser.close()
}
