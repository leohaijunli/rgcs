// End-to-end check for the heartbeat-loss handling (plan §4 L3, acceptance:
// "with the fake FC, the banner appears 3 s after the heartbeat stops; after
// recovery the state is re-read and the banner clears; screenshots").
//   node check-heartbeat.mjs <viteUrl> <shotsDir>
//
// The fake FC lives in the page: it streams telemetry snapshots, and
// `__mockHeartbeatStop()` mirrors what Rust does — no more telemetry, then,
// after `DEFAULT_HEARTBEAT_TIMEOUT` (3 s, `core::mavlink::connection`),
// a `LinkStatus { fc_alive: false }` on the `link` channel. What the check
// covers is the UI's reaction to that event, not the timeout itself (that is
// Rust's own unit territory).
//
// `mission_download` is recorded: a reconnect must re-read the mission state,
// which is the part of "restored" that is not carried by the heartbeat.
import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node check-heartbeat.mjs <viteUrl> <shotsDir>')
  process.exit(1)
}

/** Mirrors DEFAULT_HEARTBEAT_TIMEOUT in crates/core/src/mavlink/connection.rs. */
const HEARTBEAT_TIMEOUT_MS = 3000

let failed = 0
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`)
  if (!ok) failed += 1
}

// Headless Chromium renders the Cesium map on a software GL stack, which
// starves the page's own timers (a 3 s `setTimeout` lands seconds late), so
// every wait is driven from Node by polling the DOM. That also makes the
// "banner appears 3 s after the heartbeat stops" measurement a wall-clock
// number rather than a render-tick artefact.
async function waitUntil(fn, timeoutMs, stepMs = 250) {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    if (await fn()) return true
    if (Date.now() >= deadline) return false
    await page.waitForTimeout(stepMs)
  }
}

const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
const pageErrors = []
page.on('pageerror', (e) => pageErrors.push(String(e)))
page.on('console', (m) => {
  if (m.type() === 'error') pageErrors.push(m.text())
})

await page.addInitScript((heartbeatTimeoutMs) => {
  const handlers = {}
  let fired = 0
  let lossTimer = undefined

  const deliver = (event, payload) => {
    const handler = handlers[event]
    if (handler === undefined) return false
    const cb = window[`__mock_cb_${handler}`]
    if (typeof cb !== 'function') return false
    fired += 1
    cb({ event, id: fired, payload })
    return true
  }

  // The FC streaming normally: one fix a moment ago, battery, attitude.
  const snapshot = (atMs) => ({
    last_heartbeat_at_ms: atMs % 100000,
    heartbeat: {
      system_id: 1,
      component_id: 1,
      vehicle_type: 'QUADROTOR',
      autopilot: 'PX4',
      base_mode: {
        custom_mode_enabled: true,
        test_enabled: false,
        auto_enabled: false,
        guided_enabled: true,
        stabilize_enabled: true,
        hil_enabled: false,
        manual_input_enabled: false,
        safety_armed: true,
      },
      custom_mode: 4,
      mode_label: 'Hold',
      flight_state: 'Hovering',
      mavlink_version: 3,
    },
    global_position: {
      time_boot_ms: 1,
      latitude_deg: 49.2831,
      longitude_deg: -123.1207,
      altitude: { datum: 'AMSL', meters: 34 },
      relative_alt_m: 30,
      velocity: { x_m_s: 0, y_m_s: 0, z_m_s: 0 },
      heading_deg: 12,
    },
    attitude: { roll_deg: 0.3, pitch_deg: -0.4, yaw_deg: 12 },
    sys_status: { sensors: {}, battery_voltage_mv: 15400, battery_current_ma: 12000, battery_remaining_percent: 76 },
    battery: null,
    gps: { fix_type: 'RTK_FIXED', satellites_visible: 18 },
    field_ages: {
      heartbeat_at_ms: atMs,
      global_position_at_ms: atMs,
      attitude_at_ms: atMs,
      sys_status_at_ms: atMs,
      battery_at_ms: null,
      gps_at_ms: atMs,
    },
  })

  // Test hooks (Node drives the app through these).
  let feedTimer = undefined
  window.__mockCalls = []
  window.__mockUnhandled = []
  window.__mockSnapshot = snapshot
  window.__mockTelemetry = (atMs) => deliver('telemetry', snapshot(atMs))
  // A live FC: a fresh snapshot every tick. Headless Chromium clamps the page's
  // timers, so this is a floor, not a schedule — enough to keep the ages inside
  // `FIELD_STALE_MS`.
  window.__mockFeedStart = (intervalMs = 250) => {
    clearInterval(feedTimer)
    window.__mockTelemetry(Date.now())
    feedTimer = setInterval(() => window.__mockTelemetry(Date.now()), intervalMs)
  }
  window.__mockFeedStop = () => clearInterval(feedTimer)
  window.__mockLink = (status) => deliver('link', status)
  window.__mockHeartbeatStop = () => {
    const stopAt = Date.now()
    clearTimeout(lossTimer)
    // No more telemetry, then the FC is declared dead exactly as Rust does.
    lossTimer = setTimeout(() => {
      window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: false })
    }, heartbeatTimeoutMs)
    return stopAt
  }
  window.__mockHeartbeatResume = () => {
    clearTimeout(lossTimer)
    const at = Date.now()
    window.__mockTelemetry(at)
    window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: true })
    return at
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
      if (cmd === 'alert_snapshot') return []
      if (cmd === 'enumerate_devices') return []
      if (cmd === 'link_status' || cmd === 'connect' || cmd === 'link_error') return null
      // Recorded rather than ignored: the reconnect path is what we assert on.
      window.__mockCalls.push(cmd)
      if (cmd === 'mission_download') return null
      window.__mockUnhandled.push(cmd)
      return null
    },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
}, HEARTBEAT_TIMEOUT_MS)

await page.goto(url, { waitUntil: 'networkidle', timeout: 60000 })
await page.waitForTimeout(4000)
await mkdir(shotsDir, { recursive: true })

const banner = page.locator('[data-testid="heartbeat-banner"]')

// --- 0. a live FC shows no banner -------------------------------------------
await page.evaluate(() => {
  window.__mockFeedStart()
  window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: true })
})
await page.waitForTimeout(1000)
check((await banner.count()) === 0, 'no banner while the heartbeat is alive')
check(
  (await page.locator('[data-testid="link-stale"]').getAttribute('data-stale')) === 'false',
  'the nav-bar telemetry reads live while the snapshots are fresh',
)

// --- 0b. a stale fix alone is not a lost link --------------------------------
// The banner is tied to the `link` event Rust publishes after its own
// timeout — not to the age of the numbers, which is what the nav bar and the
// HUD mark instead.
await page.evaluate(() => window.__mockFeedStop())
await waitUntil(
  async () =>
    (await page.locator('[data-testid="link-stale"]').getAttribute('data-stale')) === 'true',
  12000,
)
check(
  (await page.locator('[data-testid="link-stale"]').getAttribute('data-stale')) === 'true',
  'the telemetry group goes stale once the snapshots stop',
)
check((await banner.count()) === 0, 'stale numbers alone do not raise the loss banner')

// --- 1. the heartbeat stops: the FC is declared dead 3 s later ---------------
const tStop = Date.now()
await page.evaluate(() => window.__mockHeartbeatStop())
const appeared = await waitUntil(
  async () => (await banner.count()) > 0,
  HEARTBEAT_TIMEOUT_MS + 10000,
)
const appearedAfterMs = Date.now() - tStop
check(appeared, 'the banner appears after the heartbeat stops')
check(
  appearedAfterMs >= HEARTBEAT_TIMEOUT_MS - 150,
  `the banner is not raised before the 3 s heartbeat timeout (got ${appearedAfterMs} ms)`,
)

const bannerText = await banner.innerText()
check(bannerText.includes('Heartbeat lost'), 'the banner names the condition')
const ageText = await page.locator('[data-testid="heartbeat-age"]').innerText()
check(/no heartbeat for \d+ s/.test(ageText), `the banner counts the loss (got "${ageText}")`)
const lastText = await page.locator('[data-testid="heartbeat-last"]').innerText()
check(
  lastText.includes('49.28310, -123.12070') &&
    lastText.includes('34.0 m AMSL') &&
    lastText.includes('76%'),
  `the banner carries the last fix, height and battery (got "${lastText}")`,
)

// The timer is a live clock, not a frozen number.
const firstLostMs = Number(await banner.getAttribute('data-lost-ms'))
await page.waitForTimeout(3000)
const secondLostMs = Number(await banner.getAttribute('data-lost-ms'))
check(
  secondLostMs - firstLostMs >= 1500,
  `the loss duration keeps counting (${firstLostMs} -> ${secondLostMs} ms)`,
)

// --- 2. flight commands are greyed out, with the reason, not gone -----------
const menu = page.locator('[data-testid="flight-menu-blocked"]')
check((await menu.count()) === 1, 'the Flight menu stays on the bar while the link is dead')
check(await menu.isDisabled(), 'the Flight menu is disabled')
check(
  (await menu.getAttribute('title')) === 'No link — heartbeat lost',
  `the Flight menu says why (got "${await menu.getAttribute('title')}")`,
)
await menu.click({ force: true })
await page.waitForTimeout(300)
check(
  (await page.getByRole('button', { name: 'Takeoff' }).count()) === 0,
  'clicking the greyed menu cannot arm a command',
)

// --- 3. the numbers stop looking live ---------------------------------------
await waitUntil(async () => (await page.locator('[data-testid="hud-stale"]').count()) > 0, 12000)
const hudStale = await page.locator('[data-testid="hud-stale"]').innerText()
check(/STALE \d+ s/.test(hudStale), `the HUD stamps its age (got "${hudStale}")`)
check(
  (await page.locator('[data-testid="link-stale"]').getAttribute('data-stale')) === 'true',
  'the nav-bar telemetry group is marked stale',
)
check(
  (await page.locator('[data-testid="link-stale-age"]').innerText()).length > 0,
  'the nav bar shows the age of its last sample',
)
check((await page.locator('[data-testid="map-stale"]').count()) === 1, 'the map marks the fix as old data')
await page.screenshot({ path: join(shotsDir, 'heartbeat-lost.png') })

// --- 4. the heartbeat comes back: banner clears, state is re-read -----------
await page.evaluate(() => {
  window.__mockHeartbeatResume()
  window.__mockFeedStart()
})
const cleared = await waitUntil(async () => (await banner.count()) === 0, 8000)
check(cleared, 'the banner clears when the heartbeat returns')
const restored = page.locator('[data-testid="link-restored"]')
check(
  await waitUntil(async () => (await restored.count()) > 0, 5000),
  'a "restored" notice appears',
)
check(
  (await restored.innerText()).includes('Link restored'),
  'a "restored" notice confirms the reconnect',
)
const calls = await page.evaluate(() => window.__mockCalls)
check(calls.includes('mission_download'), 'the mission state is re-read after the reconnect')
const liveAgain = await waitUntil(
  async () =>
    (await page.locator('[data-testid="link-stale"]').getAttribute('data-stale')) === 'false',
  8000,
)
check(liveAgain, 'the nav-bar telemetry is live again')
check((await page.locator('[data-testid="map-stale"]').count()) === 0, 'the map drops its old-data mark')
await page.screenshot({ path: join(shotsDir, 'heartbeat-restored.png') })

// The event log keeps a line for the reconnect (it is history, not a toast).
await page.locator('[data-testid="alert-open"]').click()
await page.waitForTimeout(500)
await page.getByRole('button', { name: 'Events' }).click()
await page.waitForTimeout(300)
const drawerText = await page.locator('[data-testid="alert-drawer"]').innerText()
check(drawerText.includes('Link restored'), 'the event log records the reconnect')

// --- 5. the notice is time-boxed --------------------------------------------
const expired = await waitUntil(async () => (await restored.count()) === 0, 20000, 500)
check(expired, 'the restored notice expires on its own')

const unhandled = await page.evaluate(() => window.__mockUnhandled)
if (unhandled.length > 0) {
  console.log(`  unmocked commands (ignored): ${[...new Set(unhandled)].join(', ')}`)
}

await browser.close()

if (pageErrors.length > 0) {
  console.error('page errors:', pageErrors.slice(0, 8))
  process.exit(2)
}
if (failed > 0) {
  console.error(`check:heartbeat FAILED — ${failed} assertion(s)`)
  process.exit(1)
}
console.log('check:heartbeat OK')
