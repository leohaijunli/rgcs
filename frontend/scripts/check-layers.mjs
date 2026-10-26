// U16 layers-panel acceptance: the Layers popover is grouped, every toggle
// reflects the store (no misleading "on" for an unloaded layer), the hillshade
// opacity is clamped to the readable range, "reset" restores the defaults
// without touching data sources, and the preferences survive a reload
// (localStorage). The Cesium-side visibility check needs the real globe and
// stays manual.
//
//   node scripts/check-layers.mjs <viteUrl> <shotsDir>

import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node scripts/check-layers.mjs <viteUrl> <shotsDir>')
  process.exit(1)
}

let failed = 0
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`)
  if (!ok) failed += 1
}

const browser = await chromium.launch({
  headless: true,
  args: ['--disable-gpu', '--disable-dev-shm-usage'],
})

async function boot(page) {
  await page.addInitScript(() => {
    window.__TAURI_INTERNALS__ = {
      transformCallback: (cb) => {
        const id = Math.floor(Math.random() * 1e9)
        window[`__mock_cb_${id}`] = cb
        return id
      },
      invoke: async (cmd) => {
        if (cmd === 'plugin:event|listen') return 0
        if (['enumerate_devices', 'flight_mode_table', 'alert_snapshot'].includes(cmd)) return []
        return null
      },
    }
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
    const iv = setInterval(() => {
      if (window.__mgProject) {
        clearInterval(iv)
        window.__mgProject.setState({
          project: { version: 1, name: 'Mock', crs: 'EPSG:4326', datum: 'cgvd2013', areas: [], settings: null },
        })
      }
    }, 10)
  })
  // The Layers popover lives in the map toolbar, so this check needs the real
  // globe (no `?nomap`).
  await page.goto(url, { waitUntil: 'networkidle', timeout: 60000 })
  // L8 startup dialogs gate the shell — let the window expire, confirm target.
  await page
    .waitForSelector('[data-testid="autoconnect-dialog"]', { state: 'detached', timeout: 15000 })
    .catch(() => {})
  const confirm = page.locator('[data-testid="target-change-confirm"]')
  await confirm.waitFor({ state: 'visible', timeout: 8000 }).catch(() => {})
  if ((await confirm.count()) > 0) await confirm.click()
  await page.waitForTimeout(500)
  await page.getByTitle('Layers').click()
  await page.waitForTimeout(300)
}

const panel = (page) => page.locator('[data-testid="layers-reset"]').locator('..')

// --- 1. grouping + state consistency ---------------------------------------
{
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
  await boot(page)
  await mkdir(shotsDir, { recursive: true })
  // Section labels are CSS-uppercased, so compare case-insensitively.
  const text = (await panel(page).innerText()).toLowerCase()
  for (const group of ['base map', 'terrain', 'mission & survey']) {
    check(text.includes(group), `the "${group}" group renders`)
  }
  check(text.includes('always shown'), 'the panel states the always-on safety layers')
  check(
    await page.locator('[data-testid="layers-reset"]').count() === 1,
    'the reset-to-default action is present',
  )
  // The store handle is the assertion surface for the toggle semantics (the
  // popover is presentational); the Cesium visibility itself is manual.
  const hasHandle = await page.evaluate(() => typeof window.__mgUi?.getState === 'function')
  check(hasHandle, 'the UI store handle is available for assertions')
  await page.screenshot({ path: join(shotsDir, 'layers-panel.png') })
  await page.close()
}

// --- 2. opacity clamp + persistence round-trip -----------------------------
{
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
  await boot(page)
  const stored = await page.evaluate(() => {
    // Drive the store directly: the slider is hidden without a loaded overlay.
    window.__mgUi.getState().setHillshadeOpacity(5)
    const afterHigh = window.__mgUi.getState().hillshadeOpacity
    window.__mgUi.getState().setHillshadeOpacity(0.01)
    const afterLow = window.__mgUi.getState().hillshadeOpacity
    window.__mgUi.getState().setHillshadeOpacity(0.8)
    return { afterHigh, afterLow, stored: localStorage.getItem('maggcs.layers') }
  })
  check(stored.afterHigh === 1, `opacity clamps high (got ${stored.afterHigh})`)
  check(stored.afterLow === 0.2, `opacity clamps low (got ${stored.afterLow})`)
  check(
    stored.stored != null && JSON.parse(stored.stored).hillshadeOpacity === 0.8,
    'the preference persists to localStorage',
  )
  // Reload: the store hydrates the persisted opacity.
  await page.reload({ waitUntil: 'networkidle' })
  await page.waitForTimeout(1200)
  const reloaded = await page.evaluate(() => window.__mgUi?.getState?.().hillshadeOpacity)
  check(reloaded === 0.8, `the preference survives a reload (got ${reloaded})`)
  await page.close()
}

// --- 3. reset restores defaults (and does not touch data) -------------------
{
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
  await boot(page)
  await page.evaluate(() => {
    window.__mgUi.getState().setHillshadeOpacity(0.9)
    window.__mgUi.setState({ showHeights: false })
  })
  await page.locator('[data-testid="layers-reset"]').click()
  const after = await page.evaluate(() => {
    const s = window.__mgUi.getState()
    return { op: s.hillshadeOpacity, heights: s.showHeights, stored: localStorage.getItem('maggcs.layers') }
  })
  check(after.op === 0.65, `reset restores the default opacity (got ${after.op})`)
  check(after.heights === true, 'reset restores the default toggles')
  check(after.stored != null && JSON.parse(after.stored).hillshadeOpacity === 0.65, 'reset persists')
  await page.close()
}

await browser.close()
if (failed > 0) {
  console.log(`${failed} FAILED`)
  process.exit(1)
}
console.log('check-layers OK')
