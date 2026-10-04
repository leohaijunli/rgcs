// Capture/compare screenshot baselines for the UI shell (Phase 0 acceptance).
//   node screenshot.mjs <url> <shotsDir> <baselinesDir>
// First run: saves shots and (if missing) baselines. Later runs: compare each
// shot against its baseline and report the differing-pixel count (pixelmatch).
import { chromium } from 'playwright'
import { mkdir, readFile, writeFile, stat } from 'node:fs/promises'
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
  { key: 'planning', label: 'Plan' },
  { key: 'flight', label: 'Flight' },
  { key: 'data', label: 'Data' },
]

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

const cases = []
for (const vp of VIEWPORTS) {
  for (const v of VIEWS) {
    cases.push({ name: `dark-${v.key}-${vp.name}`, vp, view: v })
  }
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

for (const c of cases) {
  await page.setViewportSize(c.vp)
  if (!c.light) {
    await page.evaluate((label) => {
      const btn = Array.from(document.querySelectorAll('button')).find(
        (b) => b.textContent && b.textContent.trim() === label,
      )
      if (btn) btn.click()
    }, c.view.label)
    await page.waitForTimeout(400)
  }
  await page.waitForTimeout(300)
  const buf = await capture(c.name)
  await compare(c.name, buf)
}

// Light theme.
await page.evaluate(() => {
  document.documentElement.dataset.theme = 'light'
})
await page.waitForTimeout(600)
const light = await capture('light-flight-1920x1080')
await compare('light-flight-1920x1080', light)

await browser.close()

if (errors.length > 0) {
  console.error('page errors:', errors.slice(0, 8))
  process.exit(2)
}
console.log('done')