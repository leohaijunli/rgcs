// End-to-end check for the TopBar information hierarchy (plan §7 U2,
// acceptance §7.4): at 1280 and 1920 the status group (Armed / Mode / Link)
// is complete and never occluded, every control meets the 32px normal-mode
// floor, and the narrow layout folds the low-priority health chips into
// "More" instead of dropping the status group. Also exercises the workflow
// and Tools menus, the Q-Mag branding, and the application's minimum width.
//   node check-topbar.mjs <viteUrl> <shotsDir>
import { chromium } from 'playwright'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const [url, shotsDir] = [process.argv[2], process.argv[3]]
if (!url || !shotsDir) {
  console.error('usage: node check-topbar.mjs <viteUrl> <shotsDir>')
  process.exit(1)
}

/** §7.4: normal (non-field) mode interactive targets are ≥ 32 px. */
const MIN_TARGET_PX = 32

let failed = 0
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`)
  if (!ok) failed += 1
}

const browser = await chromium.launch({ headless: true, args: ['--disable-gpu', '--disable-dev-shm-usage'] })
const pageErrors = []

async function boot(width, height) {
  const p = await browser.newPage({ viewport: { width, height } })
  p.on('pageerror', (e) => pageErrors.push(String(e)))
  p.on('console', (m) => {
    if (m.type() === 'error') pageErrors.push(m.text())
  })
  await p.addInitScript(() => {
    localStorage.setItem('maggcs.autoconnect', 'false')
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
          auto_enabled: false,
          guided_enabled: true,
          stabilize_enabled: true,
          hil_enabled: false,
          manual_input_enabled: false,
          safety_armed: true,
        },
        custom_mode: (2 << 16) | (0 << 24),
        mode_label: 'Altitude',
        flight_state: 'active',
        mavlink_version: 3,
      },
      global_position: {
        time_boot_ms: 1,
        latitude_deg: 49.2831,
        longitude_deg: -123.1207,
        altitude: { datum: 'AMSL', meters: 61.5 },
        relative_alt_m: 52,
        velocity: { x_m_s: 0, y_m_s: 0, z_m_s: 0 },
        heading_deg: 12,
      },
      attitude: { roll_deg: 0.3, pitch_deg: -0.4, yaw_deg: 12 },
      sys_status: {
        sensors: {},
        battery_voltage_mv: 15400,
        battery_current_ma: 12000,
        battery_remaining_percent: 64,
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
    window.__mockFeed = () => deliver('telemetry', snapshot(Date.now()))
    window.__mockTelemetry = (atMs) => deliver('telemetry', snapshot(atMs))
    window.__mockLink = (status) => deliver('link', status)
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
        if (cmd === 'tools_status') return []
        if (cmd === 'plugin:app|version') return '0.1.0'
        if (cmd === 'link_status' || cmd === 'connect' || cmd === 'link_error') return null
        if (cmd === 'flight_mode_table') {
          return [
            {
              option: { id: 'hold', label: 'Hold', main_mode: 4, sub_mode: 3, confirm: false, advanced: false },
              enabled: true,
              reason: null,
            },
          ]
        }
        return null
      },
    }
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
  })
  await p.goto(`${url}` + (url.includes('?') ? '&' : '?') + 'nomap', { waitUntil: 'networkidle', timeout: 60000 })
  // The forced start page (PJ3) gates the shell behind Tauri; the mock opens
  // a throwaway project so the surfaces under test render.
  await p.evaluate(() => {
    window.__mgProject?.setState({ project: { version: 1, name: 'Mock', crs: 'EPSG:4326', datum: 'cgvd2013', areas: [], settings: null } })
  })
  await p.evaluate(() => {
    window.__mockTelemetry(Date.now())
    window.__mockLink({ link_state: 'connected', endpoint: 'udpin:0.0.0.0:14550', fc_alive: true })
    setInterval(() => window.__mockTelemetry(Date.now()), 300)
  })

  // L8: confirm the (first) target change before exercising the toolbar.
  await p.waitForSelector('[data-testid="autoconnect-dialog"]', { state: 'detached', timeout: 15000 }).catch(() => {})
  const targetConfirm = p.locator('[data-testid="target-change-confirm"]')
  await targetConfirm.waitFor({ state: 'visible', timeout: 8000 }).catch(() => {})
  if ((await targetConfirm.count()) > 0) await targetConfirm.click()
  await p.waitForSelector('[data-testid="target-change-dialog"]', { state: 'detached', timeout: 5000 }).catch(() => {})

  await mkdir(shotsDir, { recursive: true })
  await waitUntilOn(p, async () => (await p.locator('[data-testid="topbar-status"]').count()) === 1, 20000)
  return p
}

async function waitUntilOn(p, fn, timeoutMs, stepMs = 100) {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    if (await fn()) return true
    if (Date.now() >= deadline) return false
    await p.waitForTimeout(stepMs)
  }
}

const box = async (p, sel) => p.locator(sel).boundingBox()
const visible = async (p, sel) => {
  const loc = p.locator(sel)
  if ((await loc.count()) === 0) return false
  return loc.first().isVisible()
}

function within(b, width, height) {
  if (b === null) return false
  return b.x >= 0 && b.y >= 0 && b.x + b.width <= width + 0.5 && b.y + b.height <= height + 0.5
}

// --- wide (1920): everything inline, status group complete -------------------
{
  const W = 1920
  const H = 1080
  const p = await boot(W, H)

  const status = await box(p, '[data-testid="topbar-status"]')
  check(within(status, W, H), `status group is fully inside the 1920 viewport`)
  check(await visible(p, '[data-testid="link-stale"]'), 'status: armed/mode block is visible')
  check(await visible(p, '[data-testid="flight-menu"]'), 'status: flight commands are visible')
  check(await visible(p, '[data-testid="mode-menu"]'), 'status: mode pill is visible')

  check(await visible(p, '[data-testid="rtk-pill"]'), 'wide: RTK pill is inline')
  check(await visible(p, '[data-testid="batt-pill"]'), 'wide: battery pill is inline')
  check(!(await visible(p, '[data-testid="topbar-more"]')), 'wide: no More fold is shown')
  check(await p.title() === 'Q-Mag', 'the application title is Q-Mag')
  check(await visible(p, '.topbar-logo-wordmark'), 'wide: the Q-Mag wordmark is visible')
  check(!(await visible(p, '.topbar-logo-mark')), 'wide: the compact Q mark is hidden')
  check(await visible(p, '[data-testid="inspector-shortcut"]'), 'wide: Inspector is an inline shortcut')
  check(await visible(p, '[data-testid="motor-test-shortcut"]'), 'wide: Motor test is an inline shortcut')

  // The status group must not be occluded by the groups to its right.
  const rtl = await box(p, '[data-testid="rtl-always"]')
  check(
    status !== null && rtl !== null && status.x + status.width <= rtl.x + 0.5,
    'status group sits clear of the safety pair (not occluded)',
  )

  await p.screenshot({ path: join(shotsDir, 'topbar-1920.png') })

  // A real keyboard walkthrough: opening focuses the current stage, arrows
  // move focus, and selecting/advancing a stage returns to the trigger.
  const workflow = p.getByTestId('workflow-open')
  await workflow.focus()
  await p.keyboard.press('ArrowDown')
  await p.getByTestId('workflow-panel').waitFor()
  const currentStage = await workflow.getAttribute('data-stage')
  check(await p.getByTestId(`workflow-stage-${currentStage}`).evaluate((el) => el === document.activeElement), 'workflow opening focuses the current stage')
  await p.keyboard.press('Home')
  check(await p.getByTestId('workflow-stage-prepare').evaluate((el) => el === document.activeElement), 'workflow Home focuses Prepare')
  await p.keyboard.press('ArrowDown')
  check(await p.getByTestId('workflow-stage-plan').evaluate((el) => el === document.activeElement), 'workflow ArrowDown focuses Plan')
  await p.keyboard.press('End')
  await p.keyboard.press('Enter')
  await p.getByTestId('workflow-panel').waitFor({ state: 'detached' })
  check(await workflow.getAttribute('data-stage') === 'review', 'workflow Enter selects Review')
  check(await workflow.evaluate((el) => el === document.activeElement), 'workflow selection restores trigger focus')
  await workflow.click()
  check(await p.getByTestId('workflow-next').isDisabled(), 'the last workflow stage disables Next')
  await p.getByTestId('workflow-previous').click()
  check(await workflow.getAttribute('data-stage') === 'fly', 'Previous selects Fly')
  await workflow.click()
  await p.screenshot({ path: join(shotsDir, 'workflow-1920.png') })
  await p.getByTestId('workflow-stage-prepare').click()
  await workflow.click()
  check(await p.getByTestId('workflow-previous').isDisabled(), 'the first workflow stage disables Previous')
  await p.getByTestId('workflow-next').click()
  check(await workflow.getAttribute('data-stage') === 'plan', 'Next selects Plan')

  const tools = p.getByTestId('tools-open')
  await tools.focus()
  await p.keyboard.press('ArrowDown')
  await p.getByTestId('tools-menu').waitFor()
  check(await p.getByTestId('tool-params').evaluate((el) => el === document.activeElement), 'Tools ArrowDown focuses the first item')
  check(await tools.getAttribute('aria-expanded') === 'true', 'Tools exposes its expanded state')
  check(await p.getByTestId('map-popout').count() === 0 && await p.getByRole('menuitem', { name: /pop out map/i }).count() === 0, 'Tools has no Pop out map entry')
  check(await p.getByTestId('tool-inspector').count() === 0 && await p.getByTestId('tool-actuator').count() === 0, 'wide: inline tools are not duplicated in the menu')
  await p.screenshot({ path: join(shotsDir, 'tools-1920.png') })
  await p.keyboard.press('End')
  check(await p.getByTestId('diagnostics-export').evaluate((el) => el === document.activeElement), 'Tools End focuses Export diagnostics')
  await p.keyboard.press('ArrowDown')
  check(await p.getByTestId('tool-params').evaluate((el) => el === document.activeElement), 'Tools arrows wrap to the first item')
  await p.keyboard.press('Escape')
  await p.getByTestId('tools-menu').waitFor({ state: 'detached' })
  check(await tools.evaluate((el) => el === document.activeElement), 'Tools Escape restores trigger focus')

  await p.getByTestId('settings-open').click()
  await p.getByRole('tab', { name: 'About', exact: true }).click()
  const about = p.getByTestId('about-body')
  check(await about.isVisible() && (await about.textContent()).includes('Q-Mag'), 'Settings About identifies Q-Mag')
  check(await about.getByTestId('company-logo-dark').isVisible(), 'About shows the company logo in dark theme')
  check(await about.getByTestId('company-logo-dark').evaluate((el) => el.complete && el.naturalWidth > 0), 'the dark company asset loads')
  await p.screenshot({ path: join(shotsDir, 'about-dark.png') })
  await p.getByRole('tab', { name: 'Appearance', exact: true }).click()
  await p.getByRole('button', { name: 'Light', exact: true }).click()
  await p.getByRole('tab', { name: 'About', exact: true }).click()
  check(await about.getByTestId('company-logo-light').isVisible() && !(await about.getByTestId('company-logo-dark').isVisible()), 'About switches to the light company logo live')
  check(await about.getByTestId('company-logo-light').evaluate((el) => el.complete && el.naturalWidth > 0), 'the light company asset loads')
  await p.screenshot({ path: join(shotsDir, 'about-light.png') })
  await p.keyboard.press('Escape')
  await p.screenshot({ path: join(shotsDir, 'topbar-light-1920.png') })
  await p.getByTestId('settings-open').click()
  await p.getByRole('tab', { name: 'Appearance', exact: true }).click()
  await p.getByRole('button', { name: 'Night red', exact: true }).click()
  await p.keyboard.press('Escape')
  await p.screenshot({ path: join(shotsDir, 'topbar-night-red-1920.png') })
  await p.close()
}

// --- narrow (1280): health folds, status survives ---------------------------
{
  const W = 1280
  const H = 800
  const p = await boot(W, H)

  const status = await box(p, '[data-testid="topbar-status"]')
  check(within(status, W, H), `status group is fully inside the 1280 viewport`)
  check(await visible(p, '[data-testid="link-stale"]'), 'narrow: status arm/mode block is visible')
  check(await visible(p, '[data-testid="flight-menu"]'), 'narrow: flight commands are visible')
  check(await visible(p, '[data-testid="mode-menu"]'), 'narrow: mode pill is visible')
  check(await visible(p, '[data-testid="rtl-always"]'), 'narrow: RTL stays visible')
  check(await visible(p, '[data-testid="hold-always"]'), 'narrow: Hold stays visible')

  check(await visible(p, '[data-testid="topbar-more"]'), 'narrow: the health chips fold into More')
  check(!(await visible(p, '[data-testid="rtk-pill"]')), 'narrow: RTK is folded away')
  check(await visible(p, '.topbar-logo-mark') && !(await visible(p, '.topbar-logo-wordmark')), 'narrow: the topbar shows only the Q mark')
  check(!(await visible(p, '[data-testid="inspector-shortcut"]')), 'narrow: the inline tool shortcuts fold away')

  // The More fold exposes the chips it took away.
  await p.locator('[data-testid="topbar-more"]').click()
  check(
    await waitUntilOn(p, async () => await visible(p, '[data-testid="topbar-more-menu"] [data-testid="rtk-pill"]'), 5000),
    'narrow: More reveals the RTK pill',
  )
  check(await visible(p, '[data-testid="batt-pill"]'), 'narrow: More reveals the battery pill')
  await p.keyboard.press('Escape')

  // Nothing overflows the bar: every right-group control ends inside it.
  const tools = await box(p, '[data-testid="tools-open"]')
  check(within(tools, W, H), 'tools button is fully inside the 1280 viewport')
  const settings = await box(p, '[data-testid="settings-open"]')
  check(within(settings, W, H), 'settings button is fully inside the 1280 viewport')

  await p.screenshot({ path: join(shotsDir, 'topbar-1280.png') })
  await p.getByTestId('tools-open').click()
  check(await visible(p, '[data-testid="tool-inspector"]') && await visible(p, '[data-testid="tool-actuator"]'), 'narrow: Tools exposes Inspector and Motor test')
  check(await p.getByTestId('map-popout').count() === 0, 'narrow: Tools has no map pop-out')
  await p.screenshot({ path: join(shotsDir, 'tools-1280.png') })
  await p.keyboard.press('Escape')

  // The Tauri minimum is 1024 px. Critical status and both safety actions
  // must still fit when the compact title and hidden project name apply.
  await p.setViewportSize({ width: 1024, height: 800 })
  await p.waitForTimeout(200)
  const controls = ['project-pill', 'workflow-open', 'flight-menu', 'mode-menu', 'rtl-always', 'hold-always', 'topbar-more', 'settings-open', 'alert-open']
  for (const id of controls) {
    check(within(await box(p, `[data-testid="${id}"]`), 1024, 800), `minimum width: ${id} stays inside the viewport`)
  }
  await p.screenshot({ path: join(shotsDir, 'topbar-1024.png') })
  await p.close()
}

// --- §7.4: every interactive TopBar control clears the normal-mode floor -----
{
  const W = 1440
  const H = 900
  const p = await boot(W, H)
  const targets = [
    'project-pill',
    'workflow-open',
    'rtl-always',
    'hold-always',
    'flight-menu',
    'mode-menu',
    'topbar-status',
    'tools-open',
    'alert-open',
    'settings-open',
  ]
  for (const id of targets) {
    const b = await box(p, `[data-testid="${id}"]`)
    const ok = b !== null && b.height >= MIN_TARGET_PX - 0.5 && b.width >= MIN_TARGET_PX - 0.5
    check(ok, `${id} clears the ${MIN_TARGET_PX}px target floor (${b ? `${Math.round(b.width)}×${Math.round(b.height)}` : 'missing'})`)
  }
  await p.close()
}

await browser.close()

if (pageErrors.length > 0) {
  console.error('page errors:', pageErrors.slice(0, 8))
  process.exit(2)
}
if (failed > 0) {
  console.error(`check:topbar FAILED — ${failed} assertion(s)`)
  process.exit(1)
}
console.log('check:topbar OK')
