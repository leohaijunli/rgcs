// Parameters table geometry regression (plan §6.2 / WP3a). Runs against the
// real React UI in browser mock mode (bundled PX4 metadata, no Tauri) and
// checks that the header and body share column origins within 1 CSS px.
//   node scripts/check-params.mjs [url] [screenshots]
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const require = createRequire(import.meta.url)
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright')
const url = process.argv[2] ?? 'http://localhost:5199/params.html'
const shots = process.argv[3]

/** Must match MIN_CENTER_W / PANE_STEP in ParamsApp.tsx. */
const MIN_CENTER_W = 460
const PANE_STEP = 8

const browser = await chromium.launch({ headless: true })
const context = await browser.newContext({ viewport: { width: 1366, height: 768 } })
const page = await context.newPage()
const errors = []
page.on('pageerror', (error) => errors.push(String(error)))

/** Left edge (x) of the direct children of a row/header element. */
const originsOf = (locator) =>
  locator.evaluate((el) => [...el.children].map((c) => c.getBoundingClientRect().x))

/** Width of each header column, in order (Name, Value, Default, Description). */
const headerColWidths = () =>
  page.$$eval('.p-thead > span', (els) => els.map((e) => e.getBoundingClientRect().width))

const widthOf = async (selector) => {
  const b = await page.locator(selector).boundingBox()
  assert.ok(b, `${selector} is present`)
  return b.width
}

const assertAligned = async (label) => {
  const head = await originsOf(page.locator('.p-thead'))
  const row = await originsOf(page.locator('.p-row').first())
  assert.equal(head.length, 4, `${label}: header has four columns`)
  assert.equal(row.length, 4, `${label}: row has four columns`)
  for (let i = 0; i < 4; i++) {
    assert.ok(
      Math.abs(head[i] - row[i]) <= 1,
      `${label}: column ${i} origin off by ${(head[i] - row[i]).toFixed(1)} px (header ${head[i].toFixed(1)}, row ${row[i].toFixed(1)})`,
    )
  }
}

const waitRows = () => page.locator('.p-row').first().waitFor()
const setSize = async (width, height, label) => {
  await page.setViewportSize({ width, height })
  await page.waitForTimeout(150)
  await assertAligned(label)
}
const assertNoOverflow = async (label) => {
  const o = await page.evaluate(() => ({
    x: document.documentElement.scrollWidth - window.innerWidth,
    y: document.documentElement.scrollHeight - window.innerHeight,
  }))
  assert.ok(o.x <= 1 && o.y <= 1, `${label}: no document overflow (${o.x}, ${o.y})`)
}

try {
  await page.goto(url, { waitUntil: 'networkidle' })
  await waitRows()
  if (shots) await mkdir(shots, { recursive: true })

  await setSize(1366, 768, '1366x768')
  await assertNoOverflow('1366x768')
  // Value/Default get a flexible share, not the old fixed 90/70 px (Params-2).
  {
    const w = await headerColWidths()
    assert.ok(w[1] >= 150, `1366x768: Value column >= 150 px (got ${w[1].toFixed(0)})`)
    assert.ok(w[2] >= 120, `1366x768: Default column >= 120 px (got ${w[2].toFixed(0)})`)
  }
  if (shots) await page.screenshot({ path: join(shots, 'params-1366x768.png') })

  await setSize(1920, 1080, '1920x1080')
  {
    const w = await headerColWidths()
    assert.ok(w[1] >= 220, `1920x1080: Value column >= 220 px (got ${w[1].toFixed(0)})`)
  }
  if (shots) await page.screenshot({ path: join(shots, 'params-1920x1080.png') })

  await setSize(900, 600, '900x600')
  await assertNoOverflow('900x600')
  if (shots) await page.screenshot({ path: join(shots, 'params-900x600.png') })

  // After a search.
  await setSize(1366, 768, '1366x768')
  await page.locator('.p-search').fill('MPC')
  await page.waitForTimeout(150)
  await waitRows()
  await assertAligned('after search')

  // After switching to a PX4 group.
  await page.locator('.p-search').fill('')
  await page.locator('.p-group', { hasText: 'All parameters' }).waitFor()
  const group = page.locator('.p-group').nth(1)
  await group.click()
  await page.waitForTimeout(150)
  await waitRows()
  await assertAligned('after group switch')

  // After deep scrolling (virtualization re-renders a different slice).
  await page.locator('.p-scroll').evaluate((el) => {
    el.scrollTop = 4000
  })
  await page.waitForTimeout(200)
  await assertAligned('after deep scroll')
  if (shots) await page.screenshot({ path: join(shots, 'params-deep-scroll.png') })

  // --- Params-3: resizable / collapsible side panes ------------------------
  await page.evaluate(() => localStorage.removeItem('maggcs.params.layout'))
  await page.reload({ waitUntil: 'networkidle' })
  await waitRows()

  // Keyboard resize of the group pane, persisted across a reload.
  await setSize(1366, 768, '1366x768 (pane resize)')
  const beforeW = await widthOf('.p-left')
  await page.locator('.p-split.left').focus()
  for (let i = 0; i < 5; i++) await page.keyboard.press('ArrowRight')
  await page.waitForTimeout(60)
  const afterW = await widthOf('.p-left')
  assert.ok(
    afterW >= beforeW + 5 * PANE_STEP - 2,
    `ArrowRight widens the group pane (${beforeW.toFixed(0)} -> ${afterW.toFixed(0)})`,
  )
  await page.reload({ waitUntil: 'networkidle' })
  await waitRows()
  const restoredW = await widthOf('.p-left')
  assert.ok(
    Math.abs(restoredW - afterW) <= 2,
    `pane width survives a reload (${afterW.toFixed(0)} -> ${restoredW.toFixed(0)})`,
  )

  // At the declared minimum size, collapsing both panes must grow the table
  // while keeping it above MIN_CENTER_W and free of overflow.
  await setSize(900, 600, '900x600 (collapsed)')
  const centerOpen = await widthOf('.p-tablewrap')
  await page.getByRole('button', { name: 'Hide group pane' }).click()
  await page.locator('.p-rail.left .rail-toggle').waitFor()
  assert.equal(await page.locator('.p-left').count(), 0, 'collapsed group pane is unmounted')
  await page.getByRole('button', { name: 'Hide editor' }).click()
  await page.locator('.p-rail.right .rail-toggle').waitFor()
  assert.equal(await page.locator('.p-detail').count(), 0, 'collapsed editor is unmounted')
  const centerCollapsed = await widthOf('.p-tablewrap')
  assert.ok(
    centerCollapsed >= MIN_CENTER_W,
    `table keeps >= ${MIN_CENTER_W} px with both panes collapsed (got ${centerCollapsed.toFixed(0)})`,
  )
  assert.ok(
    centerCollapsed > centerOpen,
    `collapsing the panes widens the table (${centerOpen.toFixed(0)} -> ${centerCollapsed.toFixed(0)})`,
  )
  await assertNoOverflow('900x600 collapsed')
  await assertAligned('900x600 collapsed')
  if (shots) await page.screenshot({ path: join(shots, 'params-collapsed.png') })
  await page.locator('.p-rail.left .rail-toggle').click()
  await page.locator('.p-left').waitFor()
  await page.locator('.p-rail.right .rail-toggle').click()
  await page.locator('.p-detail').waitFor()

  assert.deepEqual(errors, [])
  console.log('PASS: header/body column origins align within 1 px at all sizes and scroll states')
} finally {
  await browser.close()
}
