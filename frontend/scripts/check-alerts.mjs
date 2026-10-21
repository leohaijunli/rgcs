// End-to-end check for the alert center's UI (plan §3 A3, acceptance:
// "the three views agree").
//   node check-alerts.mjs <viteUrl> <shotsDir>
//
// Runs the real UI in headless Chromium with `window.__TAURI_INTERNALS__`
// mocked: the mock keeps the alert model, mirrors what Rust does on
// `alert_ack` / `alert_ack_all` (mark acked, then publish the transition) and
// delivers `alert` events into the app's real listener.
//
// What the Rust side owns (dedup window, latching, hysteresis) is covered by
// `core::alerts` unit tests; this script covers the mirror: one store feeding
// the TopBar badge, the banner stack and the drawer, and all three agreeing
// after a raise, a repeat, an acknowledge and an "ack all".
import { chromium } from 'playwright'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node check-alerts.mjs <viteUrl> <shotsDir>')
  process.exit(1)
}

let failed = 0
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`)
  if (!ok) failed += 1
}

const browser = await chromium.launch({ headless: true, args: ['--disable-gpu', '--disable-dev-shm-usage'] })
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
const pageErrors = []
page.on('pageerror', (e) => pageErrors.push(String(e)))
page.on('console', (m) => {
  if (m.type() === 'error') pageErrors.push(m.text())
})

await page.addInitScript(() => {
  const alerts = new Map()
  const handlers = {}
  let fired = 0

  const deliver = (event, payload) => {
    const handler = handlers[event]
    if (handler === undefined) return false
    const cb = window[`__mock_cb_${handler}`]
    if (typeof cb !== 'function') return false
    fired += 1
    cb({ event, id: fired, payload })
    return true
  }

  // Test hooks (Node drives the app through these).
  window.__mockFlush = () => fired
  window.__mockUnhandled = []
  window.__mockRaise = (alert, kind) => {
    alerts.set(alert.id, alert)
    return deliver('alert', { kind, alert })
  }
  // L1: a STATUSTEXT the hub publishes (already assembled, command attached
  // when it explains a rejection).
  window.__mockStatustext = (text) => deliver('statustext', text)

  // What Rust does on `alert_ack`: flip the flag, publish the transition.
  window.__mockAck = (id) => {
    const alert = alerts.get(id)
    if (!alert) return false
    const acked = { ...alert, acked: true }
    alerts.set(id, acked)
    return deliver('alert', { kind: 'acknowledged', alert: acked })
  }

  window.__TAURI_INTERNALS__ = {
    transformCallback: (cb, once = false) => {
      const id = Math.floor(Math.random() * 1e9)
      window[`__mock_cb_${id}`] = (result) => {
        cb(result)
        if (once) delete window[`__mock_cb_${id}`]
      }
      return id
    },
    invoke: async (cmd, args) => {
      if (cmd === 'plugin:event|listen') {
        handlers[args.event] = args.handler
        return fired + 1000
      }
      if (cmd.startsWith('plugin:event|')) return null
      if (cmd === 'alert_snapshot') return [...alerts.values()]
      if (cmd === 'alert_ack') {
        window.__mockAck(args.id)
        return null
      }
      if (cmd === 'alert_ack_all') {
        for (const id of [...alerts.keys()]) window.__mockAck(id)
        return null
      }
      if (cmd === 'alert_clear_acknowledged') return 0
      if (cmd === 'link_status' || cmd === 'connect' || cmd === 'link_error') return null
      // The device list is a real array in Rust; the store trusts it blindly
      // (RtkDrawer reads `.length`), so an unmocked `null` here would tear the
      // shell down instead of testing alerts.
      if (cmd === 'enumerate_devices') return []
      // Everything else the shell asks for on boot: stay alive and record it,
      // so drift shows up as a warning instead of a crash.
      window.__mockUnhandled.push(cmd)
      return null
    },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
})

await page.goto(`${url}` + (url.includes('?') ? '&' : '?') + 'nomap', { waitUntil: 'networkidle', timeout: 60000 })
await page.evaluate(() => {
  // The forced start page (PJ3) gates the shell behind Tauri; the mock opens
  // a throwaway project so the surfaces under test render.
  window.__mgProject?.setState({ project: { version: 1, name: 'Mock', crs: 'EPSG:4326', datum: 'cgvd2013', areas: [], settings: null } })
})
await page.waitForTimeout(4000)
await mkdir(shotsDir, { recursive: true })

const badge = page.locator('[data-testid="alert-badge"]')
const banner = page.locator('[data-testid="alert-banner"]')
const rows = page.locator('[data-testid="alert-row"]')

const badgeText = async () => ((await badge.count()) === 0 ? '' : (await badge.innerText()).trim())

async function waitForBadge(text) {
  await page.waitForFunction(
    (want) => {
      const el = document.querySelector('[data-testid="alert-badge"]')
      const got = el ? el.textContent.trim() : ''
      return got === want
    },
    text,
    { timeout: 5000 },
  )
}

// --- 1. three alerts, three severities --------------------------------------
const t0 = Date.now()
const critical = {
  id: 'gps|GPS lost',
  severity: 'critical',
  source: 'gps',
  text: 'GPS lost',
  first_ts_ms: t0,
  last_ts_ms: t0,
  count: 1,
  latched: true,
  acked: false,
}
const warning = {
  id: 'battery|Battery low',
  severity: 'warn',
  source: 'battery',
  text: 'Battery low',
  first_ts_ms: t0 + 1,
  last_ts_ms: t0 + 1,
  count: 1,
  latched: false,
  acked: false,
}
const info = {
  id: 'link|Endpoints reloaded',
  severity: 'info',
  source: 'link',
  text: 'Endpoints reloaded',
  first_ts_ms: t0 + 2,
  last_ts_ms: t0 + 2,
  count: 1,
  latched: false,
  acked: false,
}

for (const alert of [critical, warning, info]) {
  const delivered = await page.evaluate((a) => window.__mockRaise(a, 'raised'), alert)
  if (!delivered) console.log('  (no alert listener registered yet)')
}
await waitForBadge('3')
check((await badgeText()) === '3', 'badge counts the three alerts')
check((await banner.count()) === 1, 'banner appears for the unacked Critical alert')
check(
  (await banner.innerText()).includes('GPS lost'),
  'banner shows the Critical alert text',
)
await page.screenshot({ path: join(shotsDir, 'alerts-raised.png') })

// --- 2. a repeat counts, it does not add a row ------------------------------
const repeated = { ...warning, count: 2, last_ts_ms: t0 + 500 }
await page.evaluate((a) => window.__mockRaise(a, 'repeated'), repeated)
await page.waitForTimeout(300)
check((await badgeText()) === '3', 'a repeat does not raise the badge')

// --- 3. acknowledge the Critical from the banner ----------------------------
await page.locator('[data-testid="alert-banner-ack"]').click()
await waitForBadge('2')
check((await badgeText()) === '2', 'badge drops to 2 after ack')
check((await banner.count()) === 0, 'banner disappears after ack')
await page.screenshot({ path: join(shotsDir, 'alerts-after-ack.png') })

// --- 4. the drawer agrees with badge and banner -----------------------------
await page.locator('[data-testid="alert-open"]').click()
await page.waitForTimeout(300)
check((await rows.count()) === 3, 'drawer lists all three alerts (history kept)')
const criticalRow = page.locator(`[data-testid="alert-row"][data-alert-id="${critical.id}"]`)
check((await criticalRow.getAttribute('data-acked')) === 'true', 'the acked row is marked')
const warnRow = page.locator(`[data-testid="alert-row"][data-alert-id="${warning.id}"]`)
check((await warnRow.getAttribute('data-acked')) === 'false', 'the unacked row is open')
check((await warnRow.innerText()).includes('\u00d72'), 'the repeat shows as a count on the row')
check((await warnRow.innerText()).includes('Battery low'), 'row shows the alert text')
await page.screenshot({ path: join(shotsDir, 'alerts-drawer.png') })

// --- 5. ack all from the drawer --------------------------------------------
await page.locator('[data-testid="alert-ack-all"]').click()
await page.waitForFunction(
  () => document.querySelector('[data-testid="alert-badge"]') === null,
  undefined,
  { timeout: 5000 },
)
check((await badge.count()) === 0, 'badge is gone once nothing needs attention')
check((await rows.count()) === 3, 'drawer still shows the history')
const ackedFlags = await rows.evaluateAll((els) => els.map((e) => e.dataset.acked))
check(
  ackedFlags.every((f) => f === 'true'),
  'every row reads acknowledged',
)
await page.screenshot({ path: join(shotsDir, 'alerts-drawer-acked.png') })

// --- 6. history can be cleared ---------------------------------------------
await page.locator('[data-testid="alert-clear"]').click()
await page.waitForFunction(
  () => document.querySelectorAll('[data-testid="alert-row"]').length === 0,
  undefined,
  { timeout: 5000 },
)
check((await rows.count()) === 0, 'clear drops the acknowledged history')
// --- 7. the FC's own status texts reach the event log (L1) ------------------
// The shell delivers the assembled message exactly as Rust does; the Events
// tab must show the FC's words, tagged as coming from the FC, with the
// rejected command named.
const deliveredText = await page.evaluate(() =>
  window.__mockStatustext({
    severity: 'warning',
    text: 'Preflight Fail: no GPS',
    command: 'arm_disarm',
    at_ms: Date.now(),
  }),
)
check(deliveredText, 'the statustext listener is registered at boot')
await page.getByRole('button', { name: 'Events' }).click()
await page.waitForFunction(() => document.body.innerText.includes('arm_disarm rejected'), undefined, {
  timeout: 5000,
})
const eventsText = await page.locator('[data-testid="alert-drawer"]').innerText()
check(
  eventsText.includes('arm_disarm rejected: Preflight Fail: no GPS'),
  'a rejected command shows the FC reason it explains',
)
const sources = await page
  .locator('[data-testid="event-row"]')
  .evaluateAll((els) => els.map((e) => e.dataset.eventSource))
check(sources.includes('fc'), `the line is tagged as coming from the FC (got ${sources.join(', ')})`)
await page.screenshot({ path: join(shotsDir, 'events-statustext.png') })

const unhandled = await page.evaluate(() => window.__mockUnhandled)
if (unhandled.length > 0) console.log(`  unmocked commands (ignored): ${[...new Set(unhandled)].join(', ')}`)
await page.screenshot({ path: join(shotsDir, 'alerts-cleared.png') })

await browser.close()

if (pageErrors.length > 0) {
  console.error('page errors:', pageErrors.slice(0, 8))
  process.exit(2)
}
if (failed > 0) {
  console.error(`check:alerts FAILED — ${failed} assertion(s)`)
  process.exit(1)
}
console.log('check:alerts OK')
