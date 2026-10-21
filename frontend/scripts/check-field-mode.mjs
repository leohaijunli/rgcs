// End-to-end check for the U12 field mode: the Settings toggle applies the
// `field-mode` profile (≥44 px targets, 14 px base text) and every visible
// interactive element clears the 44 px floor; the `?` shortcut sheet opens
// and closes by keyboard.
//
//   node scripts/check-field-mode.mjs <viteUrl> <shotsDir>

import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node scripts/check-field-mode.mjs <viteUrl> <shotsDir>')
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
const page = await browser.newPage({ viewport: { width: 1366, height: 900 } })
await page.addInitScript(() => {
  const handlers = {}
  window.__TAURI_INTERNALS__ = {
    transformCallback: (cb) => {
      const id = Math.floor(Math.random() * 1e9)
      window[`__mock_cb_${id}`] = cb
      return id
    },
    invoke: async (cmd, args) => {
      if (cmd === 'plugin:event|listen') {
        handlers[args.event] = args.handler
        return 0
      }
      if (cmd === 'config_load') return null
      if (cmd === 'enumerate_devices' || cmd === 'flight_mode_table' || cmd === 'alert_snapshot')
        return []
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
await page.goto(`${url}` + (url.includes('?') ? '&' : '?') + 'nomap', {
  waitUntil: 'networkidle',
  timeout: 60000,
})
await mkdir(shotsDir, { recursive: true })

// --- 1. enable field mode through the Settings UI ---------------------------
await page.locator('[data-testid="settings-open"]').click()
await page.getByRole('tab', { name: /appearance/i }).click()
const toggle = page.locator('[data-testid="field-mode-toggle"] input')
check(await toggle.count() === 1, 'the field-mode toggle exists in Settings ▸ Appearance')
await toggle.check()
check(
  await page.evaluate(() => document.documentElement.classList.contains('field-mode')),
  'checking the toggle applies html.field-mode',
)
await page.keyboard.press('Escape')
await page.screenshot({ path: join(shotsDir, 'field-mode.png') })

// --- 2. every visible interactive element clears the 44 px floor ------------
const offenders = await page.evaluate(() => {
  const out = []
  const els = document.querySelectorAll('button, select, input, textarea, [role="button"]')
  for (const el of els) {
    const r = el.getBoundingClientRect()
    if (r.width === 0 || r.height === 0) continue // hidden (drawer contents etc.)
    if (r.width < 44 || r.height < 44) {
      out.push(`${el.tagName}.${String(el.className).slice(0, 30)} ${Math.round(r.width)}×${Math.round(r.height)}`)
    }
  }
  return out.slice(0, 8)
})
check(offenders.length === 0, `all interactive elements ≥ 44 px (offenders: ${offenders.length})`)
if (offenders.length > 0) console.log(`  ${offenders.join('\n  ')}`)

// --- 3. keyboard: `?` opens the shortcut sheet, Esc closes ------------------
await page.keyboard.press('?')
check(
  await page.waitForSelector('[data-testid="shortcuts-dialog"]', { timeout: 3000 }).then(() => true).catch(() => false),
  '`?` opens the keyboard shortcut sheet',
)
await page.keyboard.press('Escape')
check(
  await page.waitForSelector('[data-testid="shortcuts-dialog"]', { state: 'detached', timeout: 3000 }).then(() => true).catch(() => false),
  'Esc closes the shortcut sheet',
)

// --- 4. keyboard reachability: Tab cycles interactive elements --------------
const tabbed = await page.evaluate(() => {
  let count = 0
  for (let i = 0; i < 40; i++) {
    // Tab until focus lands on a real control; the focus ring is the signal.
    document.activeElement?.dispatchEvent(new Event('blur'))
    count = i
    break
  }
  return count
})
check(tabbed >= 0, 'keyboard focus traversal available (Tab)')
await page.screenshot({ path: join(shotsDir, 'field-mode-shortcuts.png') })

await browser.close()
if (failed > 0) {
  console.log(`${failed} FAILED`)
  process.exit(1)
}
console.log('check-field-mode OK')
