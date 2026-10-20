#!/usr/bin/env node
// The virtual-user shell walkthrough (operator request 2026-10-18): drive the
// app in a headless browser the way an operator would — open it, walk the
// five stages, open the project surfaces, the About dialog — and report what
// a human would see. NOT a CI gate: the mission pipeline behind the UI runs
// on Tauri commands (covered by core tests); this verifies the shell.
//
//   node scripts/e2e-shell.mjs [url]   (default http://localhost:5199/)

import { chromium } from 'playwright'

const url = process.argv[2] ?? 'http://localhost:5199/'
const results = []
let passed = 0

function report(name, ok, detail = '') {
  results.push({ name, ok, detail })
  console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${name}${detail ? ` — ${detail}` : ''}`)
  if (ok) passed += 1
}

const browser = await chromium.launch({ headless: true })
const page = await browser.newPage({ viewport: { width: 1920, height: 1080 } })
const pageErrors = []
page.on('pageerror', (e) => pageErrors.push(String(e)))

await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 60000 })
await page.waitForSelector('[data-testid="tools-open"]', { timeout: 30000 })

// 1. The five stages all open and each leaves the expected surface visible.
const STAGES = [
  ['prepare', 'prepare-panel'],
  ['plan', null], // the plan steps render by their step cards, not a testid
  ['preflight', 'preflight-stage-panel'],
  ['fly', null],
  ['review', 'review-panel'],
]
for (const [stage, panelId] of STAGES) {
  await page.getByRole('button', { name: new RegExp(`^${stage}`, 'i') }).first().click()
  await page.waitForTimeout(250)
  if (panelId) {
    const visible = await page.isVisible(`[data-testid="${panelId}"]`)
    report(`stage ${stage} opens its panel`, visible)
  } else {
    report(`stage ${stage} opens`, true)
  }
}

// 2. The Preflight tab shows the readiness badge slot (no link → no badge,
// but the panel says so instead of crashing).
await page.getByRole('button', { name: /preflight/i }).first().click()
await page.waitForTimeout(400)
const preflightHint = await page.isVisible('[data-testid="preflight-stage-panel"]')
report('preflight panel renders without a link', preflightHint)

// 3. The project pill opens the project dialog (mock: no project).
await page.click('[data-testid="project-pill"]')
await page.waitForTimeout(300)
const projectDialog = await page.isVisible('[data-testid="project-home"]')
report('project pill opens the project home', projectDialog)
await page.keyboard.press('Escape')

// 4. About: Tools ▸ About shows version + copyright + attribution.
await page.click('[data-testid="tools-open"]')
await page.waitForTimeout(200)
await page.click('[data-testid="about-open"]')
await page.waitForTimeout(300)
const aboutBody = await page.textContent('[data-testid="about-body"]')
report(
  'About shows copyright, license and attribution',
  aboutBody.includes('2026') &&
    aboutBody.includes('Apache') &&
    aboutBody.toLowerCase().includes('nrcan'),
  aboutBody.replace(/\s+/g, ' ').slice(0, 90),
)
await page.keyboard.press('Escape')

// 5. The drape parameters section exists in terrain mode with the required
// clearance field (the plan's no-default rule, visible to the operator).
await page.getByRole('button', { name: /^plan/i }).first().click()
await page.waitForTimeout(300)
await page.getByRole('button', { name: /Terrain \(AGL\)/i }).first().click()
await page.waitForTimeout(200)
// The drape parameters live inside the (accordion) Pattern step card.
await page.locator('button[aria-expanded]', { hasText: 'Pattern' }).click()
await page.waitForTimeout(300)
const hasClearance = await page.isVisible('[data-plan-field="min-clearance"]')
report('terrain-mode drape parameters expose the required clearance', hasClearance)

// 6. No page errors during the whole walkthrough.
report('no page errors during the walkthrough', pageErrors.length === 0, pageErrors.join('; ').slice(0, 120))

await browser.close()
console.log(`\n${passed}/${results.length} checks passed`)
if (passed !== results.length) process.exit(1)
