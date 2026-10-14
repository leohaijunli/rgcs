// End-to-end check for the 3D terrain toggle (plan B, task E).
//   node verify-terrain3d.mjs <viteUrl> <tilesetDir> <shotsDir>
// Runs the real UI in headless Chromium against a real generated tileset:
// an in-process HTTP server serves layer.json + .terrain (with CORS + gzip),
// and `window.__TAURI_INTERNALS__.invoke` is mocked so the Layers-popover
// toggle drives the real store → scene → CesiumTerrainProvider pipeline.
// Asserts: provider swap on/off, layer.json + tile fetches, start-failure
// rollback, a WebGL pixel diff proving the terrain mesh actually changes the
// rendered view vs the ellipsoid, and captures screenshots as evidence.
import { chromium } from 'playwright'
import { createServer } from 'node:http'
import { mkdir, readFile } from 'node:fs/promises'
import { join, normalize } from 'node:path'

const [url, tilesetDir, shotsDir] = [process.argv[2], process.argv[3], process.argv[4]]
if (!url || !tilesetDir || !shotsDir) {
  console.error('usage: node verify-terrain3d.mjs <url> <tilesetDir> <shotsDir>')
  process.exit(1)
}

// --- tileset server (CORS so the vite origin may fetch it) -------------
const root = normalize(tilesetDir)
const requests = []
const server = createServer(async (req, res) => {
  const cors = {
    'Access-Control-Allow-Origin': '*',
    'Access-Control-Allow-Methods': 'GET, OPTIONS',
    'Access-Control-Allow-Headers': 'Accept',
  }
  if (req.method === 'OPTIONS') {
    res.writeHead(204, cors).end()
    return
  }
  const path = decodeURIComponent(new URL(req.url, 'http://x').pathname)
  const file = normalize(join(root, path))
  if (!file.startsWith(root)) {
    res.writeHead(403, cors).end()
    return
  }
  try {
    const body = await readFile(file)
    const headers = { ...cors, 'Content-Length': body.length }
    if (file.endsWith('.terrain')) {
      headers['Content-Type'] = 'application/vnd.quantized-mesh'
      headers['Content-Encoding'] = 'gzip'
    } else {
      headers['Content-Type'] = 'application/json'
    }
    requests.push(path)
    console.log(`  tileset-server <- ${req.method} ${req.url}`)
    res.writeHead(200, headers).end(body)
  } catch {
    console.log(`  tileset-server <- ${req.method} ${req.url} (404)`)
    res.writeHead(404, cors).end('not found')
  }
})
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
const tilesetBase = `http://127.0.0.1:${server.address().port}/`
const layer = JSON.parse(await readFile(join(root, 'layer.json'), 'utf8'))
const [west, south, east, north] = layer.bounds
const centre = [(west + east) / 2, (south + north) / 2]

// --- browser run --------------------------------------------------------
const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } })
const pageErrors = []
page.on('pageerror', (e) => pageErrors.push(String(e)))
await page.addInitScript(
  ([base]) => {
    window.__TAURI_INTERNALS__ = {
      // Event subscriptions (stores/devices.ts) only need an id; the mock
      // invoke never fires callbacks.
      transformCallback: (cb, once = false) => {
        const id = Math.floor(Math.random() * 1e9)
        window[`__mock_cb_${id}`] = (result) => {
          cb(result)
          if (once) delete window[`__mock_cb_${id}`]
        }
        return id
      },
      invoke: async (cmd) => {
        ;(window.__mockInvokes ??= []).push(cmd)
        if (window.__mockStartFail && cmd === 'terrain3d_start') {
          throw new Error('mock start failure')
        }
        // 3D-mesh toggle now reads the DEM folder and builds from it (no
        // dialog); return the mocked source + a ready tileset.
        if (cmd === 'terrain3d_dem_source') return '/mock/dem/BC'
        if (cmd === 'terrain3d_build_from_dem') return '/mock/tileset/BC-DEMO'
        if (cmd === 'plugin:dialog|open') return '/mock/tileset/BC-DEMO'
        // Event subscriptions (devices_changed): register only, never fired.
        if (cmd === 'plugin:event|listen') return 'mock-event-id'
        if (cmd.startsWith('plugin:event|')) return null
        // Bridge boot: auto-connect attempt (bridge.ts) — pretend it
        // succeeded; the test never touches a real link.
        if (cmd === 'connect') return null
        // Bridge poll (bridge.ts): no link in the browser, like a real
        // disconnected app.
        if (cmd === 'link_status') return null
        // Tileset BASE URL (trailing slash) — fromUrl appends layer.json.
        if (cmd === 'terrain3d_start') return base
        if (cmd === 'terrain3d_stop') return null
        if (cmd === 'terrain3d_status') return null
        throw new Error(`unmocked command: ${cmd}`)
      },
    }
    // Used by event.js `_unlisten` when a listener scope ends.
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
  },
  [tilesetBase],
)

let failed = 0
const check = (ok, label) => {
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}`)
  if (!ok) failed += 1
}

// Downsampled WebGL pixel snapshot (stride 4) — used to *prove* the terrain
// mesh changes the rendered pixels vs the ellipsoid, not just that the
// provider object swapped (screenshots can't be eyeballed by automation).
const snapPixels = () =>
  page.evaluate(() => {
    const viewer = window.__mgViewer
    const gl = viewer.scene.canvas.getContext('webgl2')
    viewer.scene.render()
    const w = gl.drawingBufferWidth
    const h = gl.drawingBufferHeight
    const buf = new Uint8Array(w * h * 4)
    gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, buf)
    const out = new Array(buf.length / 4)
    for (let i = 0, o = 0; i < buf.length; i += 16, o += 1) {
      out[o] = (buf[i] << 16) | (buf[i + 1] << 8) | buf[i + 2]
    }
    return out
  })

try {
  await page.goto(url, { waitUntil: 'networkidle', timeout: 60000 })
  await page.waitForTimeout(4000)
  await mkdir(shotsDir, { recursive: true })

  // 1. The toggle renders in the Layers popover, initially OFF.
  await page.getByRole('button', { name: 'Layers', exact: true }).click()
  const toggle = page.getByRole('button', { name: /Terrain \(3D mesh\)/ })
  check(await toggle.isVisible(), 'toggle visible in Layers popover')
  check((await toggle.getAttribute('aria-pressed')) === 'false', 'toggle starts OFF')
  await page.screenshot({ path: join(shotsDir, 'terrain3d-popover-off.png') })

  // 2. Turn ON: the mocked DEM folder is read + built, server URL goes to
  // the store.
  await toggle.click()
  await page.waitForFunction(
    () => document.querySelector('[aria-pressed="true"] button[aria-pressed="true"]') != null ||
      Array.from(document.querySelectorAll('button[aria-pressed="true"]')).some((b) =>
        b.textContent?.includes('Terrain (3D mesh)'),
      ),
    undefined,
    { timeout: 15000 },
  )
  check(true, 'toggle flips ON')
  const serving = await page.getByText(/Serving/).isVisible().catch(() => false)
  check(serving, 'serving hint shown')

  // 3. Cesium swapped its terrain provider to the quantized-mesh one.
  await page.waitForFunction(
    () => window.__mgViewer?.terrainProvider?.constructor?.name === 'CesiumTerrainProvider',
    undefined,
    { timeout: 15000 },
  )
  check(true, 'CesiumTerrainProvider installed')

  // 4. Point the camera at the tileset and watch real tile requests land.
  await page.evaluate(([lon, lat]) => {
    const viewer = window.__mgViewer
    viewer.scene.morphTo3D(0)
    const Cartesian3 = viewer.camera.position.constructor
    viewer.camera.setView({
      destination: Cartesian3.fromDegrees(lon, lat, 4500),
      orientation: { heading: -0.6, pitch: -Math.PI / 6, roll: 0 },
    })
  }, centre)
  const deadline = Date.now() + 20000
  while (Date.now() < deadline && !requests.some((p) => p.endsWith('.terrain'))) {
    await page.waitForTimeout(250)
  }
  check(requests.includes('/layer.json'), 'layer.json fetched')
  check(
    requests.some((p) => p.endsWith('.terrain')),
    'at least one .terrain tile fetched',
  )
  // Evidence shots: popover ON against the rendered tileset, then the
  // globe alone (Esc closes the popover; step 5 reopens it if needed).
  await page.waitForTimeout(4000)
  await page.screenshot({ path: join(shotsDir, 'terrain3d-popover-on.png') })
  await page.keyboard.press('Escape')
  await page.waitForTimeout(300)
  await page.screenshot({ path: join(shotsDir, 'terrain3d-globe.png') })

  // 5. Turn OFF: provider falls back to the ellipsoid, server stops.
  // Both pixel snapshots must exclude the popover UI (Esc first), so the
  // only difference left is the terrain mesh vs the ellipsoid.
  await page.keyboard.press('Escape')
  await page.waitForTimeout(300)
  const terrainPx = await snapPixels()
  if (!(await toggle.isVisible().catch(() => false))) {
    await page.getByRole('button', { name: 'Layers', exact: true }).click()
  }
  await toggle.click()
  await page.waitForFunction(
    () => window.__mgViewer?.terrainProvider?.constructor?.name === 'EllipsoidTerrainProvider',
    undefined,
    { timeout: 15000 },
  )
  check(true, 'ellipsoid provider restored on OFF')
  const invokes = await page.evaluate(() => window.__mockInvokes ?? [])
  check(invokes.includes('terrain3d_stop'), 'terrain3d_stop invoked')
  // Evidence: same camera on the ellipsoid — the ON/OFF pair shows relief.
  await page.keyboard.press('Escape')
  await page.waitForTimeout(2500)
  await page.screenshot({ path: join(shotsDir, 'terrain3d-globe-ellipsoid.png') })
  const ellipsoidPx = await snapPixels()
  const pxDiff = Math.round(
    (terrainPx.reduce((a, _, i) => a + (terrainPx[i] !== ellipsoidPx[i] ? 1 : 0), 0) /
      terrainPx.length) * 1000,
  ) / 10
  check(
    pxDiff > 3,
    `terrain visibly changes the rendered pixels (${pxDiff}% > 3% vs ellipsoid)`,
  )

  // 6. Start failure rolls back: stays OFF and shows the error.
  await page.evaluate(() => {
    window.__mockStartFail = true
  })
  if (!(await toggle.isVisible().catch(() => false))) {
    await page.getByRole('button', { name: 'Layers', exact: true }).click()
  }
  await toggle.click()
  await page.waitForTimeout(1500)
  check(
    (await toggle.getAttribute('aria-pressed')) === 'false',
    'failed start keeps toggle OFF',
  )
  const errShown = await page.getByText(/mock start failure/).isVisible().catch(() => false)
  check(errShown, 'start error surfaced in popover')
  await page.screenshot({ path: join(shotsDir, 'terrain3d-popover-error.png') })

  // 7. Terrain mode drapes against the mesh (operator bug: the requested
  //    "40 m AGL" was displayed as an absolute "40 m AMSL" — the survey AGL
  //    was never measured against the terrain, and the row only ever showed
  //    AMSL). Turn the mesh back on, plan a waypoint over it, and read the row.
  await page.evaluate(() => {
    window.__mockStartFail = false
  })
  if (!(await toggle.isVisible().catch(() => false))) {
    await page.getByRole('button', { name: 'Layers', exact: true }).click()
  }
  await toggle.click()
  await page.waitForFunction(
    () => window.__mgViewer?.terrainProvider?.constructor?.name === 'CesiumTerrainProvider',
    undefined,
    { timeout: 15000 },
  )
  await page.keyboard.press('Escape')
  await page.waitForTimeout(500)

  await page.getByRole('button', { name: 'Plan', exact: true }).click()
  await page.getByRole('button', { name: /^Terrain/ }).click()
  await page.getByTitle(/Add waypoints/).click()
  const canvasBox = await page.locator('canvas').first().boundingBox()
  await page.mouse.click(canvasBox.x + canvasBox.width / 2, canvasBox.y + canvasBox.height / 2)
  // 600 ms drape debounce + mesh sampling + the store round-trip.
  await page.waitForTimeout(4000)
  const panel = page.locator('aside')
  // The mesh tiles under the new point may still be downloading on a cold
  // provider (the test toggled the mesh on seconds ago): wait for the map's
  // ground sample instead of racing it.
  await page
    .waitForFunction(() => window.__mgTerrain?.getState?.().groundBySeq != null, undefined, {
      timeout: 20000,
    })
    .catch(() => console.log('  debug: groundBySeq never populated'))
  // The step cards collapse by default; the height readouts live in step 3.
  await panel.getByRole('button', { name: /Waypoints/ }).click()
  await page.waitForTimeout(300)
  const aglRows = await panel.getByText(/^\d+\.\d m AGL$/).allTextContents()
  const amslRows = await panel.getByText(/^\d+\.\d m AMSL$/).allTextContents()
  if (aglRows.length === 0) {
    // Diagnostics: what did the drape decide, and what ground did the map
    // sample for the rows?
    const diag = await page.evaluate(() => {
      const t = window.__mgTerrain?.getState?.()
      const m = window.__mgMission?.getState?.()
      return {
        mesh3d: t?.terrain3d?.url ?? null,
        meshSampler: t?.meshSampler
          ? { ...t.meshSampler, sample: t.meshSampler.sample ? 'fn' : null }
          : null,
        groundBySeq: t?.groundBySeq ?? null,
        tile: t?.tile?.path ?? null,
        mode: m?.altitudeMode ?? null,
        wpAlt: m?.waypoints?.map((w) => w.altitude.meters) ?? null,
        meshDrape: m?.meshDrape ?? null,
        drapeError: m?.drapeError ?? null,
      }
    })
    console.log(`  debug: stores = ${JSON.stringify(diag)}`)
    const probe = await page.evaluate(async () => {
      const t = window.__mgTerrain.getState()
      const wp = window.__mgMission.getState().waypoints[0]
      const pt = [wp.position.latitude_deg, wp.position.longitude_deg]
      const h = t.meshSampler ? await t.meshSampler.sample([pt]) : null
      return { point: pt, sample: h, provider: window.__mgViewer?.terrainProvider?.constructor?.name }
    })
    console.log(`  debug: probe = ${JSON.stringify(probe)}`)
    // Diagnostics: is the panel even mounted, and what does it say?
    console.log(`  debug: aside count = ${await page.locator('aside').count()}`)
    const text = await panel.innerText().catch((e) => `<no aside: ${e}>`)
    console.log(`  debug: panel text = ${text.replace(/\n+/g, ' | ').slice(0, 1200)}`)
    console.log(`  debug: body text = ${(await page.locator('body').innerText()).replace(/\n+/g, ' | ').slice(0, 1200)}`)
  }
  check(
    aglRows.includes('40.0 m AGL'),
    `terrain drape bakes the requested 40 m AGL (got: ${aglRows.join(', ') || 'nothing'})`,
  )
  check(amslRows.length > 0, `the absolute height is still shown (${amslRows.join(', ')})`)
  await page.screenshot({ path: join(shotsDir, 'terrain3d-draped-waypoint.png') })

  check(pageErrors.length === 0, `no page errors${pageErrors.length ? `: ${pageErrors.join(' | ')}` : ''}`)
} catch (e) {
  failed += 1
  console.error('FAIL (exception)', e)
} finally {
  await browser.close()
  server.close()
}

console.log(
  failed === 0
    ? 'verify-terrain3d OK — screenshots in ' + shotsDir
    : `verify-terrain3d FAILED — ${failed} check(s) failed`,
)
process.exit(failed === 0 ? 0 : 1)
