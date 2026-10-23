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

  // --- Params-4: the two "Custom" controls are now distinct ---------------
  {
    const tabNames = await page.$$eval('.p-left-tabs .p-tab', (els) =>
      els.map((e) => e.textContent?.trim()),
    )
    const filterNames = await page.$$eval('.p-filter .p-tab', (els) =>
      els.map((e) => e.textContent?.trim()),
    )
    assert.ok(!tabNames.includes('Custom'), `pane chooser is not labelled "Custom" (${tabNames.join(', ')})`)
    assert.ok(!filterNames.includes('Custom'), `membership filter is not labelled "Custom" (${filterNames.join(', ')})`)
    assert.ok(tabNames.includes('My groups'), `pane chooser is labelled "My groups" (${tabNames.join(', ')})`)
    assert.ok(filterNames.includes('In groups'), `membership filter is labelled "In groups" (${filterNames.join(', ')})`)
  }

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

  // --- Params-5: keyboard row navigation -----------------------------------
  await setSize(1366, 768, '1366x768 (keyboard)')
  await page.locator('.p-scroll').focus()
  await page.keyboard.press('Home')
  await page.waitForTimeout(80)
  const firstSel = (await page.locator('.p-row.sel .p-name').textContent())?.trim()
  await page.keyboard.press('ArrowDown')
  await page.keyboard.press('ArrowDown')
  await page.waitForTimeout(80)
  const thirdSel = (await page.locator('.p-row.sel .p-name').textContent())?.trim()
  assert.ok(
    firstSel && thirdSel && firstSel !== thirdSel,
    `ArrowDown moves the selection (${firstSel} -> ${thirdSel})`,
  )
  assert.equal(
    (await page.locator('.p-detail-title').textContent())?.replace('\u27f2', '').trim(),
    thirdSel?.replace('\u27f2', '').trim(),
    'the detail pane follows the keyboard selection',
  )
  await page.keyboard.press('End')
  await page.waitForTimeout(150)
  const lastVisible = await page.locator('.p-row.sel').evaluate((el) => {
    const list = el.closest('.p-scroll')
    if (!list) return false
    const r = el.getBoundingClientRect()
    const l = list.getBoundingClientRect()
    return r.top >= l.top - 1 && r.bottom <= l.bottom + 1
  })
  assert.ok(lastVisible, 'End scrolls the selected row into view')

  // --- Params-5: row height follows Field-mode density ---------------------
  await page.evaluate(() => localStorage.setItem('maggcs.fieldMode', 'true'))
  await page.reload({ waitUntil: 'networkidle' })
  await waitRows()
  const density = await page.evaluate(() => {
    const inner = document.querySelector('.p-scroll > div')
    const row = document.querySelector('.p-row')
    const hit = document.querySelector('.p-hitcount')?.textContent ?? ''
    return {
      innerH: inner ? inner.getBoundingClientRect().height : 0,
      rowH: row ? row.getBoundingClientRect().height : 0,
      rows: Number(hit.split('/')[0]) || 0,
    }
  })
  assert.ok(
    Math.abs(density.rowH - 44) <= 1,
    `Field mode enlarges rows to the 44 px touch floor (got ${density.rowH.toFixed(0)})`,
  )
  assert.ok(
    Math.abs(density.innerH - density.rows * density.rowH) <= 2,
    `the virtual span matches rows x row height (${density.innerH.toFixed(0)} vs ${(density.rows * density.rowH).toFixed(0)})`,
  )

  // --- Params-6: FC status next to the title -------------------------------
  {
    assert.equal(
      await page.locator('.p-toolbar .p-title + .p-fc').count(),
      1,
      'an FC status chip sits next to the title',
    )
    assert.equal(
      (await page.locator('.p-toolbar .p-fc').textContent())?.trim(),
      'Mock',
      'browser mode reports "Mock"',
    )
    assert.equal(
      await page.locator('.p-version-ok').count(),
      0,
      'the standalone version strip was folded into the chip',
    )
  }

  // --- Params-6: the staged tray stays bounded and reachable ----------------
  await page.evaluate(() => {
    const store = window.__mgParams
    const names = [...store.getState().meta.keys()].slice(0, 40)
    store.setState({ staged: new Map(names.map((n, i) => [n, i])) })
  })
  await page.locator('.p-tray').waitFor()
  const tray = await page.evaluate(() => {
    const list = document.querySelector('.p-tray-list')
    const apply = document.querySelector('.p-tray-apply')
    return {
      listScroll: list ? list.scrollHeight : 0,
      listClient: list ? list.clientHeight : 0,
      listH: list ? list.getBoundingClientRect().height : 0,
      applyVisible: apply ? apply.getBoundingClientRect().bottom <= window.innerHeight + 1 : false,
    }
  })
  assert.ok(tray.listH <= 62, `the staged list is bounded (got ${tray.listH.toFixed(0)} px)`)
  assert.ok(tray.listScroll > tray.listClient, 'many staged entries overflow the bounded list')
  assert.ok(tray.applyVisible, 'the Apply button stays reachable')
  await assertNoOverflow('staged tray')
  if (shots) await page.screenshot({ path: join(shots, 'params-staged-tray.png') })

  assert.deepEqual(errors, [])
  console.log('PASS: header/body column origins align within 1 px at all sizes and scroll states')
} finally {
  await browser.close()
}
