// End-to-end check for the alert center UI (plan §7 U3): the full-screen red
// flash on a new Critical, the latched Critical banner, non-blocking Warning
// toasts, the drawer's severity/source filters, the FC STATUS-TEXT console and
// the time-boxed sound mute.
//   node check-alert-ux.mjs <viteUrl> <shotsDir>
//
// The red flash is driven only by a *new* Critical (a latched one present at
// boot must not re-flash), and is visual only — the mute silences the tone, it
// does not silence the flash, so a muted alarm is still seen.
import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node check-alert-ux.mjs <viteUrl> <shotsDir>')
  process.exit(1)
}

let failed = 0
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`)
  if (!ok) failed += 1
}

const browser = await chromium.launch({ headless: true, args: ['--disable-gpu', '--disable-dev-shm-usage'] })
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
// The flash is skipped under reduced-motion; pin the normal preference so the
// test exercises the moving path (a separate assertion covers reduced-motion).
await page.emulateMedia({ reducedMotion: 'no-preference' })
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
  window.__mockUnhandled = []
  window.__mockRaise = (alert, kind = 'raised') => {
    alerts.set(alert.id, alert)
    return deliver('alert', { kind, alert })
  }
  window.__mockStatustext = (text) => deliver('statustext', text)
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
      if (cmd === 'enumerate_devices') return []
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

const flash = page.locator('[data-testid="alert-flash"]')
const banner = page.locator('[data-testid="alert-banner"]')
const toast = page.locator('[data-testid="alert-toast"]')

async function waitUntil(fn, timeoutMs, stepMs = 50) {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    if (await fn()) return true
    if (Date.now() >= deadline) return false
    await page.waitForTimeout(stepMs)
  }
}

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

// --- 1. no flash before anything happens ------------------------------------
check((await flash.count()) === 0, 'no flash on a quiet screen')

// --- 2. a new Critical flashes the screen and latches a banner --------------
await page.evaluate((a) => window.__mockRaise(a), critical)
check(await waitUntil(async () => (await flash.count()) === 1, 2500), 'a new Critical triggers the full-screen red flash')
{
  const box = await flash.boundingBox()
  check(
    box !== null && box.x <= 0.5 && box.y <= 0.5 && box.width >= 1439 && box.height >= 899,
    `the flash covers the whole viewport (${box ? `${Math.round(box.width)}×${Math.round(box.height)} at ${Math.round(box.x)},${Math.round(box.y)}` : 'missing'})`,
  )
}
await page.screenshot({ path: join(shotsDir, 'alert-flash.png') })
check((await banner.count()) === 1, 'the Critical banner is shown')
check((await page.locator('[data-testid="alert-live"]').innerText()).includes('GPS lost'), 'the alert is announced to assistive tech')
check(await waitUntil(async () => (await flash.count()) === 0, 3000), 'the flash is a burst — it clears itself')

// --- 3. a Warning is a toast, not a banner or a flash -----------------------
await page.evaluate((a) => window.__mockRaise(a, 'raised'), warning)
check(await waitUntil(async () => (await toast.count()) === 1, 2500), 'a Warning raises one toast')
check((await toast.getAttribute('data-severity')) === 'warn', 'the toast is tagged Warning')
check((await banner.innerText()).includes('GPS lost'), 'the Critical banner is unchanged by the Warning')
check((await flash.count()) === 0, 'a Warning does not flash the screen')
await page.screenshot({ path: join(shotsDir, 'alert-toast.png') })

// --- 4. Info raises no toast ------------------------------------------------
await page.evaluate((a) => window.__mockRaise(a, 'raised'), info)
await page.waitForTimeout(300)
check((await toast.count()) === 1, 'an Info alert raises no toast')

// --- 5. the toast dismisses without acking ----------------------------------
await page.locator('[data-testid="alert-toast-dismiss"]').click()
check(await waitUntil(async () => (await toast.count()) === 0, 2000), 'the toast can be dismissed')

// --- 6. the drawer filters by severity and source ---------------------------
await page.locator('[data-testid="alert-open"]').click()
await page.waitForTimeout(200)
const rows = page.locator('[data-testid="alert-row"]')
check((await rows.count()) === 3, 'the drawer lists all three alerts')
await page.locator('[data-testid="alert-filter-critical"]').click()
check(await waitUntil(async () => (await rows.count()) === 1, 2000), 'the severity filter narrows to Critical')
check((await rows.first().getAttribute('data-severity')) === 'critical', 'the remaining row is the Critical one')
await page.screenshot({ path: join(shotsDir, 'alert-filter.png') })
await page.locator('[data-testid="alert-filter-all"]').click()
await waitUntil(async () => (await rows.count()) === 3, 2000)
const sourceFilter = page.locator('[data-testid="alert-filter-source"]')
check((await sourceFilter.count()) === 1, 'a source filter appears once there is more than one source')
await sourceFilter.selectOption('gps')
check(await waitUntil(async () => (await rows.count()) === 1, 2000), 'the source filter narrows to GPS')
await sourceFilter.selectOption('all')
await waitUntil(async () => (await rows.count()) === 3, 2000)

// --- 7. the console tab shows the FC's own status texts ---------------------
await page.evaluate(() =>
  window.__mockStatustext({
    severity: 'warning',
    text: 'Preflight Fail: no GPS',
    command: 'arm_disarm',
    at_ms: Date.now(),
  }),
)
await page.locator('[data-testid="alert-tab-console"]').click()
check(
  await waitUntil(async () => (await page.locator('[data-testid="console-line"]').count()) === 1, 3000),
  'the console lists the FC status text',
)
check(
  (await page.locator('[data-testid="console-line"]').first().innerText()).includes('Preflight Fail'),
  'the console shows the FC words',
)
await page.screenshot({ path: join(shotsDir, 'alert-console.png') })
await page.locator('[data-testid="alert-tab-alerts"]').click()

// --- 8. mute is time-boxed, and does not silence the flash ------------------
await page.locator('[data-testid="alert-mute"]').click()
check((await page.locator('[data-testid="alert-mute"]').getAttribute('aria-pressed')) === 'true', 'mute engages')
check((await page.locator('[data-testid="alert-mute"]').innerText()).includes('Unmute'), 'the button offers to unmute while muted')
check((await page.locator('[data-testid="alert-volume"]').count()) === 1, 'volume is adjustable')
// A second Critical while muted: the tone is silenced, the flash is not.
await page.evaluate((a) => window.__mockRaise({ ...a, id: 'rtk|RTK lost', text: 'RTK lost', severity: 'critical', source: 'rtk' }, 'raised'), critical)
check(await waitUntil(async () => (await flash.count()) === 1, 2500), 'a muted Critical still flashes the screen')
await page.screenshot({ path: join(shotsDir, 'alert-muted-flash.png') })
await page.locator('[data-testid="alert-mute"]').click()
check((await page.locator('[data-testid="alert-mute"]').getAttribute('aria-pressed')) === 'false', 'mute releases')

// --- 9. reduced motion suppresses the flash ---------------------------------
await page.emulateMedia({ reducedMotion: 'reduce' })
await waitUntil(async () => (await flash.count()) === 0, 3000)
await page.evaluate((a) => window.__mockRaise({ ...a, id: 'mag|Mag spike', text: 'Mag spike', severity: 'critical', source: 'mag' }, 'raised'), critical)
await page.waitForTimeout(600)
check((await flash.count()) === 0, 'prefers-reduced-motion suppresses the flash')

const unhandled = await page.evaluate(() => window.__mockUnhandled)
if (unhandled.length > 0) console.log(`  unmocked commands (ignored): ${[...new Set(unhandled)].join(', ')}`)

await browser.close()

if (pageErrors.length > 0) {
  console.error('page errors:', pageErrors.slice(0, 8))
  process.exit(2)
}
if (failed > 0) {
  console.error(`check:alert-ux FAILED — ${failed} assertion(s)`)
  process.exit(1)
}
console.log('check:alert-ux OK')
