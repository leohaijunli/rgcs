import { chromium } from 'playwright'

const url = 'http://127.0.0.1:8931/inspector.html'
const browser = await chromium.launch()
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } })
const errors = []
page.on('console', (m) => { if (m.type() === 'error') errors.push('CONSOLE: ' + m.text()) })
page.on('pageerror', (e) => errors.push('PAGEERROR: ' + e.message))

await page.goto(url, { waitUntil: 'load' })
await page.waitForTimeout(1500)
await page.screenshot({ path: '/tmp/inspector-after-1.5s.png' })
// Click a signal checkbox, then watch for crashes.
const checkboxes = page.locator('.signal-list input[type=checkbox]')
console.log('checkbox count:', await checkboxes.count())
if ((await checkboxes.count()) > 0) {
  await checkboxes.first().check()
  await page.waitForTimeout(2000)
  await page.screenshot({ path: '/tmp/inspector-after-check.png' })
  console.log('plot count:', await page.locator('.plot').count())
}
await page.waitForTimeout(1500)
await page.screenshot({ path: '/tmp/inspector-final.png' })
console.log('errors:')
for (const e of errors) console.log(' ', e)
console.log('total errors:', errors.length)
await browser.close()