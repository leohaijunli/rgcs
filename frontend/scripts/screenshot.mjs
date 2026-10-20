// Capture/compare screenshot baselines for the UI shell (UI6).
//   node screenshot.mjs <url> <shotsDir> <baselinesDir>
// Matrix: dark|light × 1366x768|1920x1080 × Plan|Flight (8 baselines).
// Theme switches go through localStorage + reload so the boot path applies
// it (React store, canvas instruments and secondary mirrors included).
// First run: saves shots and (if missing) baselines. Later runs: compare each
// shot against its baseline and report the differing-pixel count (pixelmatch).
import { chromium } from 'playwright'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import pixelmatch from 'pixelmatch'
import { PNG } from 'pngjs'

const [url, shotsDir, baselinesDir] = [process.argv[2], process.argv[3], process.argv[4]]
if (!url || !shotsDir) {
  console.error('usage: node screenshot.mjs <url> <shotsDir> [baselinesDir]')
  process.exit(1)
}
const BASELINES = baselinesDir ?? join(shotsDir, 'baselines')

const VIEWPORTS = [
  { name: '1920x1080', width: 1920, height: 1080 },
  { name: '1366x768', width: 1366, height: 768 },
]
const VIEWS = [
  { key: 'prepare', label: 'Prepare' },
  { key: 'plan', label: 'Plan' },
  { key: 'preflight', label: 'Preflight' },
  { key: 'fly', label: 'Fly' },
  { key: 'review', label: 'Review' },
]
const THEMES = ['dark', 'light']

const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } })
const errors = []
page.on('pageerror', (e) => errors.push(String(e)))
page.on('console', (m) => {
  if (m.type() === 'error') errors.push(m.text())
})

await page.goto(url, { waitUntil: 'networkidle', timeout: 60000 })
await page.waitForTimeout(4000)
await mkdir(shotsDir, { recursive: true })
await mkdir(BASELINES, { recursive: true })

async function setTheme(theme) {
  await page.evaluate((t) => localStorage.setItem('maggcs.theme', t), theme)
  await page.reload({ waitUntil: 'networkidle', timeout: 60000 })
  await page.waitForTimeout(4000)
}

async function gotoView(label) {
  await page.evaluate((label) => {
    const btn = Array.from(document.querySelectorAll('button')).find(
      (b) => b.textContent && b.textContent.trim() === label,
    )
    if (btn) btn.click()
  }, label)
  await page.waitForTimeout(400)
}

async function capture(name) {
  const buf = await page.screenshot({ buffer: true })
  const shot = join(shotsDir, `${name}.png`)
  await writeFile(shot, buf)
  return buf
}

async function compare(name, buf) {
  const base = join(BASELINES, `${name}.png`)
  try {
    const baseBuf = await readFile(base)
    const a = PNG.sync.read(baseBuf)
    const b = PNG.sync.read(buf)
    if (a.width !== b.width || a.height !== b.height) {
      console.log(`  ${name}: SIZE MISMATCH (${a.width}x${a.height} vs ${b.width}x${b.height})`)
      return
    }
    const diff = new PNG({ width: a.width, height: a.height })
    const n = pixelmatch(a.data, b.data, diff.data, a.width, a.height, { threshold: 0.12 })
    const diffFile = join(shotsDir, `diff-${name}.png`)
    await writeFile(diffFile, PNG.sync.write(diff))
    console.log(`  ${name}: ${n} differing pixels (${((n / (a.width * a.height)) * 100).toFixed(3)}%)`)
  } catch {
    // No baseline yet: create it.
    await writeFile(join(BASELINES, `${name}.png`), buf)
    console.log(`  ${name}: baseline created`)
  }
}

async function warmup() {
  // Run the viewport/view matrix once without capturing: Cesium tiles and
  // route lines need a few seconds after each theme reload, and the first
  // shot would otherwise baseline a blank map.
  for (const vp of VIEWPORTS) {
    await page.setViewportSize(vp)
    for (const v of VIEWS) {
      await gotoView(v.label)
      await page.waitForTimeout(800)
    }
  }
}

for (const theme of THEMES) {
  await setTheme(theme)
  await warmup()
  for (const vp of VIEWPORTS) {
    await page.setViewportSize(vp)
    for (const v of VIEWS) {
      await gotoView(v.label)
      const name = `${theme}-${v.key}-${vp.name}`
      const buf = await capture(name)
      await compare(name, buf)
    }
  }
}

await browser.close()

if (errors.length > 0) {
  console.error('page errors:', errors.slice(0, 8))
  process.exit(2)
}
console.log('done')
