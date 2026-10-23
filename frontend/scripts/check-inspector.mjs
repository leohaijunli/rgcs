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
/** MIN_CHART_H in InspectorApp.tsx. */
const MIN_CHART_H = 160

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
const heightOf = async (selector) => {
  const b = await box(selector)
  assert.ok(b, `${selector} is present`)
  return b.height
}
/** Number of resolved grid tracks in the plot workspace (Insp-2). */
const gridCols = () =>
  page
    .locator('.plot-grid')
    .evaluate((el) => getComputedStyle(el).gridTemplateColumns.split(' ').filter(Boolean).length)
/** The layout preset <select> is the second select in the toolbar. */
const layoutSelect = () => page.locator('.toolbar select').nth(1)
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

  // --- Insp-3: common header + grouped toolbar + dedicated replay strip ----
  {
    assert.equal(
      (await page.locator('.tool-header .tool-title').textContent())?.trim(),
      'Signal Inspector',
      'the tool title sits in the common header',
    )
    assert.equal(
      (await page.locator('.tool-header .source-chip').textContent())?.trim(),
      'Mock',
      'the header source chip identifies the mock source',
    )
    assert.equal(
      (await page.locator('.tool-header .tool-status').textContent())?.trim(),
      'synthetic telemetry',
      'status text sits in the header, not the toolbar',
    )
    const groups = await page.$$eval('.toolbar .tool-group', (els) =>
      els.map((e) => e.getAttribute('aria-label')),
    )
    assert.deepEqual(
      groups,
      ['Playback', 'Source', 'Plots', 'Workspace'],
      `toolbar controls are grouped (got ${groups.join(', ')})`,
    )
    assert.equal(await page.locator('.replay-strip').count(), 0, 'no replay strip when not replaying')
  }

  // --- Insp-4: signal groups and trace selection are keyboard reachable ----
  {
    const toggle = page.locator('.signal-list .group-toggle').first()
    assert.ok(await toggle.count(), 'each signal group exposes a disclosure button')
    assert.equal(await toggle.getAttribute('aria-expanded'), 'true', 'groups start expanded')
    const before = await page.locator('.signal-list label').count()
    await toggle.focus()
    await page.keyboard.press('Enter')
    await page.waitForTimeout(100)
    assert.equal(await toggle.getAttribute('aria-expanded'), 'false', 'Enter collapses the group')
    assert.ok(
      (await page.locator('.signal-list label').count()) < before,
      'collapsing a group hides its signals',
    )
    await page.keyboard.press('Space')
    await page.waitForTimeout(100)
    assert.equal(await toggle.getAttribute('aria-expanded'), 'true', 'Space re-expands the group')

    const chipName = page.locator('.plot .trace-chip .chip-name').first()
    assert.ok(await chipName.count(), 'trace chips expose a focusable selector')
    await chipName.focus()
    await page.keyboard.press('Enter')
    await page.waitForTimeout(100)
    assert.equal(await chipName.getAttribute('aria-pressed'), 'true', 'Enter selects the trace')
    assert.ok(
      (await page.locator('.plot .trace-chip.selected').count()) >= 1,
      'the selected trace is highlighted',
    )
  }

  // --- Insp-5: notice severity levels never displace the workspace ---------
  {
    const gridBefore = await page.locator('.plot-grid').boundingBox()
    for (const [severity, role] of [
      ['info', 'status'],
      ['success', 'status'],
      ['warn', 'alert'],
      ['error', 'alert'],
    ]) {
      await page.evaluate((s) => window.__mgInspector.showNotice(`${s} message`, s), severity)
      await page.waitForTimeout(50)
      const notice = page.locator('.inspector-notice')
      assert.ok(await notice.count(), `a ${severity} notice is shown`)
      assert.ok(
        (await notice.getAttribute('class')).includes(severity),
        `the ${severity} notice carries its severity class`,
      )
      assert.equal(await notice.getAttribute('role'), role, `the ${severity} notice uses role=${role}`)
      const gridAfter = await page.locator('.plot-grid').boundingBox()
      assert.ok(
        Math.abs(gridAfter.y - gridBefore.y) < 1 &&
          Math.abs(gridAfter.height - gridBefore.height) < 1,
        `a ${severity} notice does not displace the workspace`,
      )
    }
    if (shots) await page.screenshot({ path: join(shots, 'inspector-notice-1366x768.png') })
    await page.locator('.inspector-notice .notice-close').click()
    await page.waitForTimeout(50)
    assert.equal(await page.locator('.inspector-notice').count(), 0, 'the notice can be dismissed')
  }

  // --- 1366 x 768 ---------------------------------------------------------
  await setSize(1366, 768, '1366x768')
  if (shots) await page.screenshot({ path: join(shots, 'inspector-1366x768.png') })

  // --- 1920 x 1080 --------------------------------------------------------
  await setSize(1920, 1080, '1920x1080')
  // Insp-2: the plot fills the tall workspace and 3 columns are allowed here.
  const chartTall = await heightOf('.chart')
  assert.ok(
    chartTall > 300,
    `1920x1080: the chart fills the workspace height (got ${chartTall.toFixed(0)} px)`,
  )
  await layoutSelect().selectOption('3')
  await page.waitForTimeout(120)
  assert.equal(await gridCols(), 3, '1920x1080: a 3-column layout is used')
  assert.equal(
    await layoutSelect().locator('option[value="3"]').isDisabled(),
    false,
    '1920x1080: the 3-column option is enabled',
  )
  if (shots) await page.screenshot({ path: join(shots, 'inspector-1920x1080.png') })

  // --- 800 x 480 (declared tool minimum): center stays usable -------------
  await setSize(800, 480, '800x480')
  const centerAtMin = await widthOf('.plot-grid')
  assert.ok(
    centerAtMin >= MIN_CENTER_W - 1,
    `800x480: the plot workspace keeps >= ${MIN_CENTER_W} px (got ${centerAtMin.toFixed(0)})`,
  )
  // Insp-2: the chart keeps its floor and the column count caps to 1.
  const chartMin = await heightOf('.chart')
  assert.ok(
    chartMin >= MIN_CHART_H - 1,
    `800x480: the chart keeps >= ${MIN_CHART_H} px (got ${chartMin.toFixed(0)})`,
  )
  assert.equal(await gridCols(), 1, '800x480: the column count caps to 1')
  assert.equal(
    await layoutSelect().locator('option[value="3"]').isDisabled(),
    true,
    '800x480: the 3-column option is disabled',
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
