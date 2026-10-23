// Signal Inspector layout regression (plan §6.1 / WP8). Runs against the real
// React UI in browser mock mode (no Tauri APIs, no FC) and checks the resizable
// / collapsible sidebars keep the plot workspace usable and bounded.
//   node scripts/check-inspector.mjs [url] [screenshots]
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const require = createRequire(import.meta.url)
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright')
const url = process.argv[2] ?? 'http://localhost:5199/inspector.html'
const shots = process.argv[3]

/** Must match MIN_CENTER_W in InspectorApp.tsx. */
const MIN_CENTER_W = 240
/** PANE_STEP in InspectorApp.tsx. */
const PANE_STEP = 8

const browser = await chromium.launch({ headless: true })
const context = await browser.newContext({ viewport: { width: 1366, height: 768 } })
const page = await context.newPage()
const errors = []
page.on('pageerror', (error) => errors.push(String(error)))

const box = (selector) => page.locator(selector).boundingBox()
const widthOf = async (selector) => {
  const b = await box(selector)
  assert.ok(b, `${selector} is present`)
  return b.width
}
/** No document-level overflow: the grid must not grow past the viewport. */
const assertNoOverflow = async (label) => {
  const overflow = await page.evaluate(() => ({
    x: document.documentElement.scrollWidth - window.innerWidth,
    y: document.documentElement.scrollHeight - window.innerHeight,
  }))
  assert.ok(overflow.x <= 1, `${label}: no horizontal overflow (${overflow.x} px)`)
  assert.ok(overflow.y <= 1, `${label}: no vertical overflow (${overflow.y} px)`)
}
const setSize = async (width, height, label) => {
  await page.setViewportSize({ width, height })
  await page.waitForTimeout(120)
  await assertNoOverflow(label)
}

try {
  await page.goto(url, { waitUntil: 'networkidle' })
  await page.locator('.inspector-grid').waitFor()
  // Let a few mock samples land so a plot exists (checks the center track too).
  await page.getByRole('checkbox').first().check()
  await page.waitForTimeout(300)
  if (shots) await mkdir(shots, { recursive: true })

  // --- 1366 x 768 ---------------------------------------------------------
  await setSize(1366, 768, '1366x768')
  if (shots) await page.screenshot({ path: join(shots, 'inspector-1366x768.png') })

  // --- 1920 x 1080 --------------------------------------------------------
  await setSize(1920, 1080, '1920x1080')
  if (shots) await page.screenshot({ path: join(shots, 'inspector-1920x1080.png') })

  // --- 800 x 480 (declared tool minimum): center stays usable -------------
  await setSize(800, 480, '800x480')
  const centerAtMin = await widthOf('.plot-grid')
  assert.ok(
    centerAtMin >= MIN_CENTER_W - 1,
    `800x480: the plot workspace keeps >= ${MIN_CENTER_W} px (got ${centerAtMin.toFixed(0)})`,
  )
  const grid = await box('.plot-grid')
  const props = await box('.properties')
  assert.ok(props && grid, 'both panes exist')
  assert.ok(grid.x + grid.width <= props.x + 1, 'the workspace does not overlap the properties pane')
  if (shots) await page.screenshot({ path: join(shots, 'inspector-800x480.png') })

  // --- Keyboard resize persists across a reload ---------------------------
  await setSize(1366, 768, '1366x768')
  const before = await widthOf('.signal-list')
  await page.locator('.pane-splitter.left').focus()
  for (let i = 0; i < 5; i++) await page.keyboard.press('ArrowRight')
  await page.waitForTimeout(60)
  const after = await widthOf('.signal-list')
  assert.ok(
    after >= before + 5 * PANE_STEP - 2,
    `ArrowRight widens the signal list (${before.toFixed(0)} -> ${after.toFixed(0)})`,
  )
  await page.reload({ waitUntil: 'networkidle' })
  await page.locator('.inspector-grid').waitFor()
  const restored = await widthOf('.signal-list')
  assert.ok(
    Math.abs(restored - after) <= 2,
    `the dragged width survives a reload (${after.toFixed(0)} -> ${restored.toFixed(0)})`,
  )

  // --- Collapse / reopen both sidebars ------------------------------------
  await page.getByRole('button', { name: 'Hide signal list' }).click()
  await page.locator('.pane-rail.left .rail-toggle').waitFor()
  assert.equal(await page.locator('.signal-list').count(), 0, 'collapsed left pane is unmounted')
  const centerCollapsed = await widthOf('.plot-grid')
  assert.ok(
    centerCollapsed > centerAtMin,
    `collapsing the left pane grows the workspace (${centerAtMin.toFixed(0)} -> ${centerCollapsed.toFixed(0)})`,
  )
  if (shots) await page.screenshot({ path: join(shots, 'inspector-collapsed.png') })
  await page.locator('.pane-rail.left .rail-toggle').click()
  await page.locator('.signal-list').waitFor()
  assert.ok(
    Math.abs((await widthOf('.signal-list')) - restored) <= 2,
    'reopening restores the remembered width',
  )

  await page.getByRole('button', { name: 'Hide properties' }).click()
  await page.locator('.pane-rail.right .rail-toggle').waitFor()
  assert.equal(await page.locator('.properties').count(), 0, 'collapsed right pane is unmounted')
  await page.locator('.pane-rail.right .rail-toggle').click()
  await page.locator('.properties').waitFor()

  assert.deepEqual(errors, [])
  console.log('PASS: resizable/collapsible inspector sidebars stay bounded and usable')
} finally {
  await browser.close()
}
