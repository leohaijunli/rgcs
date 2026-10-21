// End-to-end check for the graded command interaction (plan §7 U4, acceptance:
// "a stray single click must not send a medium- or high-risk command; RTL is
// at most two interactions from any stage").
//   node check-command-ux.mjs <viteUrl> <shotsDir>
//
// The fake FC lives in the page: it streams telemetry, answers
// `flight_mode_table`, and records every command the UI hands to the backend
// (`__mockCommands`). Command acks are driven from Node through the `command`
// event, exactly the payload Rust publishes (app-tauri `command_service`), so
// what is checked is the UI's confirmation model — how much deliberate effort
// a command costs and what the status strip says about it.
//
// High risk (force-arm L5, quit/disconnect L4) is the keyword tier and lives in
// `check-teardown.mjs`; this file covers the click and hold tiers.
import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node check-command-ux.mjs <viteUrl> <shotsDir>')
  process.exit(1)
}

/** Mirrors HOLD_MS in frontend/src/design-system/components.tsx. */
const HOLD_MS = 1000
/** Deliberately longer than HOLD_MS so the hold is a hold, not a race. */
const HOLD_AT_MS = 1600

let failed = 0
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`)
  if (!ok) failed += 1
}

// Headless Chromium renders the Cesium map on a software GL stack, which
// starves the page's own timers, so every wait is driven from Node by polling
// the DOM (`waitUntil`) instead of trusting a page-side timeout.
let page = null
async function waitUntil(fn, timeoutMs, stepMs = 100) {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    if (await fn()) return true
    if (Date.now() >= deadline) return false
    await page.waitForTimeout(stepMs)
  }
}

const browser = await chromium.launch({ headless: true, args: ['--disable-gpu', '--disable-dev-shm-usage'] })
page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
const pageErrors = []
page.on('pageerror', (e) => {
  pageErrors.push(String(e))
  console.log(`  page error: ${e}`)
})
page.on('console', (m) => {
  if (m.type() === 'error') pageErrors.push(m.text())
})

await page.addInitScript(() => {
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

  // PX4 custom_mode packing, same as the UI's (ADR-017).
  const pack = (main, sub) => ((main & 0xff) << 16) | ((sub & 0xff) << 24)

  let armed = true
  let customMode = pack(4, 3) // Auto · Loiter
  let modeLabel = 'Hold'
  let feedTimer = undefined

  const snapshot = (atMs) => ({
    last_heartbeat_at_ms: atMs % 100000,
    heartbeat: {
      system_id: 1,
      component_id: 1,
      vehicle_type: 'quadrotor',
      autopilot: 'px4',
      base_mode: {
        custom_mode_enabled: true,
        test_enabled: false,
        auto_enabled: true,
        guided_enabled: true,
        stabilize_enabled: true,
        hil_enabled: false,
        manual_input_enabled: false,
        safety_armed: armed,
      },
      custom_mode: customMode,
      mode_label: modeLabel,
      flight_state: 'active',
      mavlink_version: 3,
    },
    global_position: {
      time_boot_ms: 1,
      latitude_deg: 49.2831,
      longitude_deg: -123.1207,
      altitude: { datum: 'AMSL', meters: 48.5 },
      relative_alt_m: 40,
      velocity: { x_m_s: 0, y_m_s: 0, z_m_s: 0 },
      heading_deg: 12,
    },
    attitude: { roll_deg: 0.3, pitch_deg: -0.4, yaw_deg: 12 },
    sys_status: {
      sensors: {},
      battery_voltage_mv: 15400,
      battery_current_ma: 12000,
      battery_remaining_percent: 76,
    },
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

  window.__mockCommands = []
  window.__mockUnhandled = []
  window.__mockFeedStart = (intervalMs = 250) => {
    clearInterval(feedTimer)
    window.__mockTelemetry(Date.now())
    feedTimer = setInterval(() => window.__mockTelemetry(Date.now()), intervalMs)
  }
  window.__mockFeedStop = () => clearInterval(feedTimer)
  window.__mockTelemetry = (atMs) => deliver('telemetry', snapshot(atMs))
  window.__mockLink = (status) => deliver('link', status)
  // Readback for a mode switch: the heartbeat is the truth (ADR-017).
  window.__mockMode = (main, sub, label) => {
    customMode = pack(main, sub)
    modeLabel = label
    window.__mockTelemetry(Date.now())
  }
  window.__mockArm = (next) => {
    armed = next
    window.__mockTelemetry(Date.now())
  }
  window.__mockCommandEvent = (payload) => deliver('command', payload)

  const modeEntry = (id, label, main, sub, confirm) => ({
    option: { id, label, main_mode: main, sub_mode: sub, confirm, advanced: false },
    enabled: true,
    reason: null,
  })

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
      if (cmd === 'flight_mode_table') {
        return [
          modeEntry('hold', 'Hold', 4, 3, false),
          modeEntry('mission', 'Mission', 4, 4, false),
          modeEntry('return', 'Return', 4, 5, true),
          modeEntry('land', 'Land', 4, 6, true),
        ]
      }
      window.__mockCommands.push({ cmd, ...(args ?? {}) })
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
await page.evaluate(() => {
  window.__mockFeedStart()
  window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: true })
})
// L8: the startup dialogs gate the shell — let the auto-connect window
// expire and confirm the (first) target change before anything clicks.
await page.waitForSelector('[data-testid="autoconnect-dialog"]', { state: 'detached', timeout: 15000 }).catch(() => {})
const targetConfirm = page.locator('[data-testid="target-change-confirm"]')
await targetConfirm.waitFor({ state: 'visible', timeout: 8000 }).catch(() => {})
if ((await targetConfirm.count()) > 0) await targetConfirm.click()
await page.waitForSelector('[data-testid="target-change-dialog"]', { state: 'detached', timeout: 5000 }).catch(() => {})

await mkdir(shotsDir, { recursive: true })
await waitUntil(async () => (await page.locator('[data-testid="flight-menu"]').count()) === 1, 20000)

/** Every command the UI handed to the backend (boot-time invokes excluded). */
const commands = () =>
  page.evaluate(() =>
    window.__mockCommands.filter((c) =>
      ['send_command', 'send_set_mode', 'send_arm_disarm'].includes(c.cmd),
    ),
  )
const status = page.locator('[data-testid="command-status"]')

/** Press and hold a button for `ms` (with an optional mid-hold snapshot). */
async function hold(locator, ms, midShot = null) {
  await locator.hover()
  await page.mouse.down()
  if (midShot) {
    await page.waitForTimeout(450)
    await midShot()
  }
  await page.waitForTimeout(ms)
  await page.mouse.up()
}

// --- 0. idle: no command, no strip ------------------------------------------
check((await commands()).length === 0, 'nothing is sent before the operator acts')
check((await status.count()) === 0, 'the status strip is empty until a command exists')

// --- 1. the menu carries the state a command will run against ---------------
await page.locator('[data-testid="flight-menu"]').click()
const bar = page.locator('[data-testid="confirm-bar"]')
check((await bar.count()) === 1, 'the confirm bar appears above the flight commands')
const barMode = await page.locator('[data-testid="confirm-mode"]').innerText()
const barAgl = await page.locator('[data-testid="confirm-agl"]').innerText()
const barBatt = await page.locator('[data-testid="confirm-battery"]').innerText()
check(barMode.includes('MODE') && barMode.includes('Hold'), `the bar names the mode (got "${barMode}")`)
// 48.5 m AMSL, 40 m above HOME; no DEM is loaded in this run, so the HOME
// plane is the AGL reference (cesium/heights.ts) and the height is 40.0 m.
check(/AGL\s+40\.0 m/.test(barAgl), `the bar names the height above ground (got "${barAgl}")`)
check(barBatt.includes('76%'), `the bar names the battery (got "${barBatt}")`)
await page.screenshot({ path: join(shotsDir, 'command-menu.png') })

// --- 2. a single click cannot send any flight command (mis-tap safety) ------
for (const key of ['takeoff', 'land', 'rtl', 'arm_disarm']) {
  await page.locator(`[data-testid="flight-cmd-${key}"]`).click()
  await page.waitForTimeout(150)
}
check(
  (await commands()).length === 0,
  'one click on each of Takeoff / Land / RTL / Arm sends nothing',
)
check(
  (await status.count()) === 0,
  'no command status strip appears for a click that sent nothing',
)

// --- 3. a hold short of the window still sends nothing ----------------------
await hold(page.locator('[data-testid="flight-cmd-land"]'), 350)
await page.waitForTimeout(200)
check((await commands()).length === 0, 'a hold released before the window sends nothing')

// --- 4. the full hold sends, with the action shown while it runs -----------
let midHold = null
await hold(page.locator('[data-testid="flight-cmd-land"]'), HOLD_AT_MS, async () => {
  midHold = await page.locator('[data-testid="flight-cmd-land"]').innerText()
  const dash = await page
    .locator('[data-testid="flight-cmd-land"] rect')
    .getAttribute('stroke-dasharray')
  check(
    (await page.locator('[data-testid="flight-cmd-land"]').getAttribute('data-holding')) === 'true',
    'the button reports the hold in progress',
  )
  check(
    Number.parseFloat(dash ?? '0') > 0,
    `the progress ring fills while holding (dasharray ${dash})`,
  )
  await page.screenshot({ path: join(shotsDir, 'command-holding.png') })
})
check(
  midHold !== null && midHold.includes('Land on the ground below'),
  `the hold names what it will execute (got "${midHold}")`,
)
const afterHold = await commands()
check(
  afterHold.some((c) => c.cmd === 'send_command' && c.name === 'land'),
  'the completed hold sends the land command',
)
check(afterHold.length === 1, `exactly one command goes out (got ${afterHold.length})`)

// --- 5. the status strip: sent -> awaiting ack -> result + elapsed ----------
await page.evaluate(() => window.__mockCommandEvent({ command: 'land', kind: 'sent' }))
check(await waitUntil(async () => (await status.count()) === 1, 5000), 'the status strip appears')
const sentText = await status.innerText()
check(sentText.includes('Land'), `the strip names the command (got "${sentText}")`)
check(
  sentText.includes('Sent — awaiting ack'),
  `the strip shows the awaiting-ack phase (got "${sentText}")`,
)
const sentAt = Date.now()
await page.waitForTimeout(900)
await page.evaluate(() =>
  window.__mockCommandEvent({ command: 'land', kind: 'completed', result: 'accepted' }),
)
check(
  await waitUntil(async () => (await status.innerText()).includes('Accepted'), 5000),
  'the strip shows the terminal ack',
)
const elapsedText = await page.locator('[data-testid="command-status-elapsed"]').innerText()
const elapsedMs = Number.parseFloat(elapsedText) * 1000
check(
  Number.isFinite(elapsedMs) && Date.now() - sentAt - elapsedMs < 1500,
  `the strip reports the round-trip time (got "${elapsedText}")`,
)
check(
  (await status.getAttribute('data-command')) === 'Land',
  'the strip keeps the command name after the ack',
)
await page.screenshot({ path: join(shotsDir, 'command-result.png') })

// --- 6. RTL is at most two interactions, from the Fly stage -----------------
await page.keyboard.press('Escape')
await page.waitForTimeout(250)
let interactions = 0
await page.locator('[data-testid="flight-menu"]').click()
interactions += 1
await hold(page.locator('[data-testid="flight-cmd-rtl"]'), HOLD_AT_MS)
interactions += 1
const rtl = await commands()
check(
  rtl.some((c) => c.cmd === 'send_command' && c.name === 'rtl'),
  'RTL goes out from the flight stage',
)
check(interactions <= 2, `RTL costs at most two interactions (got ${interactions})`)
await page.evaluate(() =>
  window.__mockCommandEvent({ command: 'rtl', kind: 'completed', result: 'accepted' }),
)
await page.keyboard.press('Escape')
await page.waitForTimeout(250)

// --- 7. the keyboard is a first-class hold path -----------------------------
const armCount = async () =>
  (await commands()).filter((c) => c.cmd === 'send_arm_disarm').length
const armButton = page.locator('[data-testid="flight-cmd-arm_disarm"]')
await page.locator('[data-testid="flight-menu"]').click()
await armButton.focus()
await page.keyboard.down('Enter')
await page.waitForTimeout(HOLD_AT_MS)
await page.keyboard.up('Enter')
check((await armCount()) === 1, 'holding Enter sends arm/disarm')
await page.evaluate(() =>
  window.__mockCommandEvent({ command: 'arm_disarm', kind: 'completed', result: 'accepted' }),
)
await page.keyboard.press('Escape')
await page.waitForTimeout(250)
check((await armCount()) === 1, 'a single Enter press after the hold does not send again')

// --- 8. mode switching follows the same grading -----------------------------
await page.locator('[data-testid="mode-menu"]').click()
const setModeCount = async () => (await commands()).filter((c) => c.cmd === 'send_set_mode').length
check((await page.locator('[data-testid="confirm-bar"]').count()) === 1, 'the mode popover carries the confirm bar')

// Low risk (Hold): one click.
await page.locator('[data-testid="mode-hold"]').click()
check(await waitUntil(async () => (await setModeCount()) === 1, 5000), 'Hold switches on a single click')
const firstMode = (await commands()).find((c) => c.cmd === 'send_set_mode')
check(
  firstMode?.mainMode === 4 && firstMode?.subMode === 3,
  `the switch carries the PX4 mode pair (got ${firstMode?.mainMode}/${firstMode?.subMode})`,
)
await page.evaluate(() => {
  window.__mockCommandEvent({ command: 'set_mode', kind: 'completed', result: 'accepted' })
  window.__mockMode(4, 3, 'Hold')
})
await waitUntil(async () => (await page.locator('[data-testid="mode-switching"]').count()) === 0, 8000)

// Medium risk (Return): a click is not enough, a hold is.
await page.locator('[data-testid="mode-return"]').click()
await page.waitForTimeout(300)
check((await setModeCount()) === 1, 'a single click cannot switch into Return')
await hold(page.locator('[data-testid="mode-return"]'), HOLD_AT_MS)
check(await waitUntil(async () => (await setModeCount()) === 2, 5000), 'holding Return switches the mode')
const returnMode = (await commands()).filter((c) => c.cmd === 'send_set_mode')[1]
check(
  returnMode?.mainMode === 4 && returnMode?.subMode === 5,
  `Return sends the RTL mode pair (got ${returnMode?.mainMode}/${returnMode?.subMode})`,
)
await page.evaluate(() => {
  window.__mockCommandEvent({ command: 'set_mode', kind: 'completed', result: 'accepted' })
  window.__mockMode(4, 5, 'RTL')
})

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
  console.error(`check:command-ux FAILED — ${failed} assertion(s)`)
  process.exit(1)
}
console.log('check:command-ux OK')
