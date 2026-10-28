#!/usr/bin/env node
// Full-log Inspector regression against the real React/uPlot UI with isolated
// IPC. Native dense-ULog decoding/decimation is tested in inspector_history.rs.
// PLAYWRIGHT_MODULE=/path/to/playwright node scripts/check-inspector-history.mjs [url] [screenshots]
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
let phase = 'startup'
page.on('pageerror', (error) => errors.push(`${phase}: ${error.stack ?? String(error)}`))

await page.addInitScript(() => {
  let callbackId = 0
  const callbacks = new Map()
  const signal = (field) => ({ system_id: 1, component_id: 1, message_id: 9001, field })
  const trace = { id: 'trace-x', signal: signal('x'), pipeline: [], analyzer: null, analyzer_source: 'raw' }
  // A saved rolling window and a selected trace must not override the fresh
  // log's whole-history default.
  localStorage.setItem('maggcs.inspector.workspace', JSON.stringify({
    version: 2, windowSec: 10, plots: [{ id: 'plot-x', title: 'Recorded signals', traces: [trace] }],
    colors: {}, axisLink: false,
  }))
  const bench = window.__inspectorHistoryBench = {
    replay: { active: false, name: null, duration_ms: 0, position_ms: 0, playing: false, speed: 1 },
    nextName: 'dense.ulg', nextDuration: 180_000, traces: [trace],
    requests: [], controls: [], pending: [], defer: false, fail: false, channel: null,
  }
  const history = (traces) => ({
    name: bench.replay.name,
    duration_ms: bench.replay.duration_ms,
    traces: traces.map((trace) => {
      const n = bench.replay.duration_ms === 0 ? 1 : 4096
      const t = Array.from({ length: n }, (_, i) => i * bench.replay.duration_ms / Math.max(1, n - 1))
      const raw = t.map((_, i) => trace.signal.field === 'x'
        ? i === 20 ? 250 : i === n - 20 ? -250 : 30 * Math.sin(i / 80)
        : 20 * Math.cos(i / 60))
      return { trace_id: trace.id, t, raw, filtered: raw.map(() => null) }
    }),
  })
  bench.feed = (endMs) => bench.channel?.onmessage({
    seq: 1, spectra: [],
    traces: bench.traces.map((trace) => ({
      trace_id: trace.id, t: [endMs - 1000, endMs], raw: [1, 2], filtered: [null, null],
    })),
  })
  bench.resolveOldHistory = () => bench.pending.shift()?.resolve()
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} }
  window.__TAURI_INTERNALS__ = {
    transformCallback(callback) { callbacks.set(++callbackId, callback); return callbackId },
    unregisterCallback(id) { callbacks.delete(id) },
    async invoke(command, args) {
      if (command === 'plugin:event|listen') return args.handler
      if (command === 'inspector_connect') { bench.channel = args.channel; return null }
      if (command === 'inspector_set_traces') { bench.traces = args.traces; return null }
      if (command === 'inspector_catalog') return ['x', 'y'].map((field) => ({
        signal: signal(field), message_name: 'sensor', last_value: 0, rate_hz: 1000,
      }))
      if (command === 'inspector_status') return {
        messages: 100, samples: 100, dropped: 0, connected: true,
        tap_running: !bench.replay.active, replay_active: bench.replay.active,
      }
      if (command === 'inspector_replay_status') return structuredClone(bench.replay)
      if (command === 'plugin:dialog|open') return `/mock/${bench.nextName}`
      if (command === 'inspector_open_ulog') {
        bench.replay = {
          active: true, name: bench.nextName, duration_ms: bench.nextDuration,
          position_ms: 0, playing: true, speed: 1,
        }
        return { name: bench.nextName, duration_ms: bench.nextDuration, topics: [{ name: 'sensor', signals: 2 }] }
      }
      if (command === 'inspector_ulog_history') {
        bench.requests.push(structuredClone(args))
        if (bench.fail) throw 'ULog history could not be read'
        const result = history(args.traces)
        if (bench.defer) return new Promise((resolve) => bench.pending.push({ resolve: () => resolve(result) }))
        return result
      }
      if (command === 'inspector_replay') {
        bench.controls.push(structuredClone(args))
        if (args.seekMs !== undefined) bench.replay.position_ms = args.seekMs
        if (args.play !== undefined) bench.replay.playing = args.play
        if (args.speed !== undefined) bench.replay.speed = args.speed
        return structuredClone(bench.replay)
      }
      if (command === 'inspector_use_live') {
        bench.replay.active = false
        bench.replay.name = null
        return null
      }
      if (['inspector_list_algorithms', 'alert_snapshot'].includes(command)) return []
      if (command === 'inspector_source') return bench.replay.active ? `ulog:${bench.replay.name}` : 'live'
      return null
    },
  }
})

async function waitFor(check, label) {
  const deadline = Date.now() + 5000
  while (!await check()) {
    assert.ok(Date.now() < deadline, label)
    await page.waitForTimeout(50)
  }
}
const windowSelect = page.locator('select[title="Time window"]')
const historyReady = () => waitFor(
  async () => (await page.getByTestId('inspector-history-status').textContent())?.trim() === 'Full log',
  'full history finishes loading',
)
const chart = () => page.evaluate(() => {
  const plot = [...window.__mgInspector.charts.values()][0]
  const x = plot?.data[0] ?? []
  return {
    min: plot?.scales.x.min, max: plot?.scales.x.max,
    first: x[0], last: x[x.length - 1], points: x.length,
    series: plot?.data.slice(1).map((data) => ({
      min: Math.min(...data.filter((v) => Number.isFinite(v))),
      max: Math.max(...data.filter((v) => Number.isFinite(v))),
    })) ?? [],
  }
})
const wholeRange = async (seconds) => {
  await waitFor(async () => {
    const value = await chart()
    return value.min === 0 && value.max === seconds && value.points > 0
  }, `the plot covers 0–${seconds} s`)
}
const capture = async (name) => { if (shots) await page.screenshot({ path: join(shots, name) }) }

try {
  if (shots) await mkdir(shots, { recursive: true })
  await page.goto(url, { waitUntil: 'networkidle' })
  await page.locator('.plot .u-over').waitFor()
  assert.equal(await windowSelect.inputValue(), '10', 'saved live window is loaded')
  await page.evaluate(() => window.__inspectorHistoryBench.feed(20_000))
  await waitFor(async () => (await chart()).points > 0, 'live samples render before source replacement')
  await page.getByRole('button', { name: 'Pause', exact: true }).click()
  await page.evaluate(() => [...window.__mgInspector.charts.values()][0].setScale('x', { min: 15, max: 20 }))

  phase = 'open full history'
  await page.getByRole('button', { name: 'Open ULog', exact: true }).click()
  await historyReady()
  assert.equal(await windowSelect.inputValue(), 'full', 'loading a ULog selects Full log')
  await wholeRange(180)
  const initial = await chart()
  assert.equal(initial.first, 0, 'beginning is visible before playback reaches it')
  assert.equal(initial.last, 180, 'end is visible without waiting for playback')
  assert.equal(initial.series[0].max, 250, 'an early peak survives display decimation')
  assert.equal(initial.series[0].min, -250, 'a late peak survives display decimation')
  assert.equal(await page.evaluate(() => window.__inspectorHistoryBench.replay.position_ms), 0,
    'history loading does not move the replay playhead')
  assert.ok(await page.evaluate(() => window.__inspectorHistoryBench.requests[0].maxPoints <= 65_536),
    'the history request carries a bounded point budget')

  phase = 'add signal'
  await page.locator('.signal-list label').filter({ hasText: /^y$/ }).getByRole('checkbox').check()
  await waitFor(async () => (await chart()).series.length === 2 && (await chart()).points > 0,
    'a signal added after loading receives its whole history')
  await historyReady()
  await wholeRange(180)
  await capture('inspector-full-history-1366x768.png')

  phase = 'zoom and seek'
  const over = await page.locator('.plot .u-over').boundingBox()
  assert.ok(over)
  await page.mouse.move(over.x + over.width * 0.2, over.y + over.height / 2)
  await page.mouse.down()
  await page.mouse.move(over.x + over.width * 0.55, over.y + over.height / 2, { steps: 10 })
  await page.mouse.up()
  await waitFor(async () => (await chart()).max - (await chart()).min < 100, 'drag-zoom narrows the full history')
  const zoom = await chart()
  await page.evaluate(() => window.__inspectorHistoryBench.feed(60_000))
  await page.waitForTimeout(200)
  assert.deepEqual([(await chart()).min, (await chart()).max], [zoom.min, zoom.max],
    'playback frames preserve the full-history zoom')
  const yCheck = page.locator('.signal-list label').filter({ hasText: /^y$/ }).getByRole('checkbox')
  await yCheck.uncheck()
  await waitFor(async () => (await chart()).series.length === 1 && (await chart()).points > 0, 'removing a trace rebuilds the zoomed plot')
  await yCheck.check()
  await waitFor(async () => (await chart()).series.length === 2 && (await chart()).points > 0, 'adding a trace rebuilds the zoomed plot')
  await historyReady()
  assert.deepEqual([(await chart()).min, (await chart()).max], [zoom.min, zoom.max],
    'trace changes preserve the full-history zoom')
  await page.getByRole('slider', { name: 'Seek replay', exact: true }).focus()
  await page.keyboard.press('End')
  await waitFor(async () => page.evaluate(() => window.__inspectorHistoryBench.controls.length > 0), 'seek is sent')
  assert.equal(await page.evaluate(() => window.__inspectorHistoryBench.controls.at(-1).spanMs), 0,
    'full-history seek moves only playback, without a rolling scrub')
  assert.deepEqual([(await chart()).min, (await chart()).max], [zoom.min, zoom.max], 'seeking preserves the plot zoom')
  await capture('inspector-history-zoom.png')
  await page.getByRole('button', { name: 'Follow newest data', exact: true }).click()
  await wholeRange(180)

  phase = 'rolling and live source'
  await windowSelect.selectOption('30')
  await waitFor(async () => (await chart()).max - (await chart()).min === 30, 'explicit rolling view uses 30 s')
  await page.evaluate(() => { window.__inspectorHistoryBench.defer = true })
  await windowSelect.selectOption('full')
  await waitFor(async () => page.evaluate(() => window.__inspectorHistoryBench.pending.length === 1), 'old history is pending')
  await page.getByRole('button', { name: 'Back to live', exact: true }).click()
  await page.evaluate(() => window.__inspectorHistoryBench.resolveOldHistory())
  await page.waitForTimeout(200)
  assert.equal(await windowSelect.inputValue(), '30', 'returning to live restores a rolling window')
  assert.equal(await page.evaluate(() => window.__mgInspector.historyBuffers().size), 0,
    'a delayed old-log reply cannot restore history after the source changes')
  assert.equal(await page.locator('.replay-strip').count(), 0)

  phase = 'failed history'
  await page.evaluate(() => {
    Object.assign(window.__inspectorHistoryBench, { defer: false, fail: true, nextName: 'short.ulg', nextDuration: 5000 })
  })
  await page.getByRole('button', { name: 'Open ULog', exact: true }).click()
  await page.locator('.inspector-notice.error').waitFor()
  assert.ok((await page.locator('.inspector-notice.error').innerText()).includes('could not be read'))
  assert.equal((await chart()).points, 0, 'a failed new-log history never displays the old recording')
  await page.getByRole('button', { name: 'Back to live', exact: true }).click()
  phase = 'short log and resize'
  await page.evaluate(() => { window.__inspectorHistoryBench.fail = false })
  await page.getByRole('button', { name: 'Open ULog', exact: true }).click()
  await historyReady()
  await wholeRange(5)
  await capture('inspector-short-log.png')
  await page.setViewportSize({ width: 800, height: 480 })
  await page.waitForTimeout(100)
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), 'full-log controls fit the minimum window')
  await capture('inspector-full-history-800x480.png')

  await page.getByRole('button', { name: 'Back to live', exact: true }).click()
  phase = 'single-point log'
  await page.evaluate(() => { window.__inspectorHistoryBench.nextDuration = 0 })
  await page.getByRole('button', { name: 'Open ULog', exact: true }).click()
  await historyReady()
  await wholeRange(0.001)
  assert.equal((await chart()).points, 1, 'a one-sample log still renders with a finite time axis')
  assert.deepEqual(errors, [])
  console.log('PASS: full 180 s history by default; peaks, added signals, zoom/seek/trace edits, rolling/live, stale/error replies, short/single-point logs')
} finally {
  await browser.close()
}
