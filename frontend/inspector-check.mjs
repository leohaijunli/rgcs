import { chromium } from 'playwright'

// `?fault` makes the mock emit NaN samples and null catalog values, mirroring
// what the real Tauri path produces when serde_json turns NaN into null
// (plan S5). A clean run must be free of console/page errors.
const url = 'http://127.0.0.1:8931/inspector.html?fault'
const browser = await chromium.launch()
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } })
const errors = []
page.on('console', (m) => { if (m.type() === 'error') errors.push('CONSOLE: ' + m.text()) })
page.on('pageerror', (e) => errors.push('PAGEERROR: ' + e.message))

// A failing step should not abort the run: keep going so we still report all
// errors, then fail the process at the end.
const step = async (name, fn) => {
  try { await fn() } catch (e) { errors.push(`STEP ${name}: ${e.message}`) }
}

await page.goto(url, { waitUntil: 'load' })
await page.waitForTimeout(1500)
await page.screenshot({ path: '/tmp/inspector-after-1.5s.png' })

const checkboxes = page.locator('.signal-list input[type=checkbox]')
console.log('checkbox count:', await checkboxes.count())
await step('check signal', async () => {
  if ((await checkboxes.count()) > 0) await checkboxes.first().check()
})
await page.waitForTimeout(1500)

// Checking a second signal must add a trace to the SAME (active) plot — no
// per-plot "+ Signal" menu.
await step('checkbox adds to active plot', async () => {
  const before = await page.locator('.plot').first().locator('.trace-chip').count()
  await checkboxes.nth(1).check()
  await page.waitForTimeout(400)
  const after = await page.locator('.plot').first().locator('.trace-chip').count()
  if (after !== before + 1) throw new Error(`expected ${before + 1} traces, got ${after}`)
})

// Per-signal colour picker sitting right after the checkbox.
await step('signal color picker', async () => {
  await page.locator('.signal-list .color-swatch').first().click()
  await page.waitForTimeout(200)
  const dots = page.locator('.color-pop .color-dot')
  const n = await dots.count()
  if (n === 0) throw new Error('no colour palette')
  await dots.nth(3).click()
  await page.waitForTimeout(300)
})

// Select the trace, attach the FFT analyzer and switch its input to the
// filtered signal (exercises the NaN-carrying filter path).
await step('select + fft', async () => {
  await page.locator('.trace-chip .mono').first().click()
  const fft = page.locator('.trace-chip button', { hasText: 'FFT' }).first()
  await fft.click()
  await page.waitForTimeout(800)
  await page.locator('.trace-chip select').first().selectOption('filtered')
  await page.waitForTimeout(1200)
})
await page.screenshot({ path: '/tmp/inspector-fft.png' })

// Add two filter stages from the Properties panel, then shrink the window.
await step('filter + window', async () => {
  const add = page.getByRole('button', { name: '+ Add filter stage' })
  await add.first().click()
  await page.waitForTimeout(400)
  await add.first().click()
  await page.waitForTimeout(400)
  await page.locator('.toolbar select').first().selectOption({ index: 0 })
})
await page.waitForTimeout(1000)

// P7: layout presets, dual cursor Δt/Δy, drag-signal-into-plot.
await step('layout 2 cols', async () => {
  await page.locator('.toolbar select').nth(1).selectOption('2')
  await page.waitForTimeout(400)
})
await step('delta cursor', async () => {
  await page.getByRole('button', { name: 'Δ cursors' }).click()
  const box = await page.locator('.chart').first().boundingBox()
  if (!box) throw new Error('no chart box')
  const y = box.y + box.height * 0.5
  await page.mouse.move(box.x + box.width * 0.35, y)
  await page.mouse.click(box.x + box.width * 0.35, y)
  await page.mouse.move(box.x + box.width * 0.7, y)
  await page.waitForTimeout(250)
  const text = await page.locator('.measure-readout').first().innerText()
  if (!/Δt/.test(text)) throw new Error(`no Δt readout: ${JSON.stringify(text)}`)
})
await page.screenshot({ path: '/tmp/inspector-p7.png' })
await step('drag signal to plot', async () => {
  await page.locator('.signal-list label').nth(3).dragTo(page.locator('.plot').first())
  await page.waitForTimeout(400)
})

// Clear while under fault injection, then keep buffering.
await step('clear', async () => {
  await page.getByRole('button', { name: 'Clear data' }).click()
})
await page.waitForTimeout(2000)
await page.screenshot({ path: '/tmp/inspector-final.png' })

console.log('plot count:', await page.locator('.plot').count())
console.log('errors:')
for (const e of errors) console.log(' ', e)
console.log('total errors:', errors.length)
await browser.close()
if (errors.length > 0) process.exit(1)
