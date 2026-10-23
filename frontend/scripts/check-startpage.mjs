// Start-page regression against the real React UI with an isolated IPC stub.
// No Tauri APIs or project files are touched. Usage:
//   node scripts/check-startpage.mjs [url] [screenshots]
// The page is previewed in the browser with ?start; the no-project workspace
// is reached through the Continue without a project action.
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'

const require = createRequire(import.meta.url)
const { chromium } = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright')
const url = process.argv[2] ?? 'http://localhost:5199/?start&nomap'
const shots = process.argv[3]
const browser = await chromium.launch({ headless: true })
const context = await browser.newContext({ viewport: { width: 1366, height: 768 } })
const page = await context.newPage()
const errors = []
page.on('pageerror', (error) => errors.push(String(error)))

const RECENT = { path: '/tmp/opencode/ProjA', name: 'ProjA' }
await page.addInitScript((recent) => {
  localStorage.setItem('maggcs.recentProjects', JSON.stringify([recent]))
  localStorage.setItem('maggcs.autoconnect', 'false')
  window.__calls = []
  window.__TAURI_INTERNALS__ = {
    transformCallback: () => 1,
    unregisterCallback: () => {},
    async invoke(command, args) {
      window.__calls.push({ command, args })
      if (command === 'plugin:event|listen') return args?.handler
      return undefined
    },
  }
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
}, RECENT)

const removed = () =>
  page.evaluate(() => JSON.parse(localStorage.getItem('maggcs.recentProjects') || '[]').length)
const backendDeletes = () =>
  page.evaluate(() => window.__calls.filter((c) => c.command === 'project_delete').length)

try {
  await page.goto(url, { waitUntil: 'networkidle' })
  await page.getByTestId('start-page').waitFor()
  if (shots) await mkdir(shots, { recursive: true })
  if (shots) await page.screenshot({ path: join(shots, 'start-page.png') })

  // Name-only create form.
  await page.getByRole('button', { name: 'New project', exact: true }).click()
  await page.getByTestId('project-wizard').waitFor()
  assert.equal(await page.getByTestId('project-name').count(), 1, 'the name field is present')
  assert.equal(await page.locator('input[aria-label="CRS"]').count(), 0, 'no CRS field')
  assert.equal(
    await page.locator('select[aria-label="Vertical datum"]').count(),
    0,
    'no vertical-datum field',
  )
  assert.equal(
    await page.getByRole('button', { name: 'Change location…', exact: true }).count(),
    1,
    'change location is available',
  )
  assert.equal(
    await page
      .locator('[data-testid="project-wizard"] input, [data-testid="project-wizard"] select')
      .count(),
    1,
    'only the name is an input',
  )
  if (shots) await page.screenshot({ path: join(shots, 'start-wizard.png') })
  await page.getByRole('button', { name: 'Cancel', exact: true }).click()

  // Remove a recent entry from the list only.
  await page.getByRole('button', { name: 'Remove ProjA from the list', exact: true }).click()
  await page.getByText('No recent projects', { exact: true }).waitFor()
  assert.equal(await removed(), 0, 'remove from list clears localStorage')
  assert.equal(await backendDeletes(), 0, 'remove from list never touches disk')

  // Delete from disk requires typing the exact project name.
  await page.reload({ waitUntil: 'networkidle' })
  await page.getByRole('button', { name: 'Delete ProjA from disk', exact: true }).click()
  await page.getByTestId('project-delete-confirm').waitFor()
  const confirmButton = page.getByRole('button', { name: 'Delete from disk', exact: true })
  assert.equal(await confirmButton.isDisabled(), true, 'the delete is disabled before typing')
  await page.getByTestId('project-delete-name').fill('ProjA-typo')
  assert.equal(await confirmButton.isDisabled(), true, 'a wrong name keeps delete disabled')
  await page.getByTestId('project-delete-name').fill('ProjA')
  assert.equal(await confirmButton.isEnabled(), true, 'the exact name enables delete')
  await confirmButton.click()
  await page.getByText('No recent projects', { exact: true }).waitFor()
  assert.equal(await backendDeletes(), 1, 'delete from disk reaches the backend once')
  assert.equal(await removed(), 0, 'delete from disk drops the recent entry')

  // Continue into the workspace with no project.
  await page.getByRole('button', { name: 'Continue without a project', exact: true }).click()
  await page.getByTestId('project-pill').waitFor()
  assert.equal(await page.getByTestId('start-page').count(), 0, 'start page is dismissed')
  assert.equal(
    await page.getByTestId('project-name').textContent(),
    'No project',
    'the workspace runs with no project',
  )

  // Branding plus five recent projects must not push the form above the
  // viewport at the desktop minimum, even with 44 px Field-mode controls.
  const compact = await browser.newPage({ viewport: { width: 1024, height: 700 } })
  compact.on('pageerror', (error) => errors.push(String(error)))
  await compact.addInitScript(() => {
    localStorage.setItem('maggcs.autoconnect', 'false')
    localStorage.setItem('maggcs.fieldMode', 'true')
    localStorage.setItem('maggcs.theme', 'light')
    localStorage.setItem('maggcs.recentProjects', JSON.stringify(
      Array.from({ length: 5 }, (_, i) => ({ name: `Survey ${i + 1}`, path: `/tmp/opencode/survey-${i + 1}` })),
    ))
  })
  const preview = new URL(url)
  preview.searchParams.set('start', '')
  preview.searchParams.set('nomap', '')
  await compact.goto(preview.href, { waitUntil: 'networkidle' })
  await compact.getByRole('button', { name: 'New project', exact: true }).click()
  await compact.getByTestId('start-page').evaluate((root) => { root.scrollTop = 0 })
  const logo = await compact.getByTestId('start-page').getByTestId('app-logo').boundingBox()
  assert.ok(logo && logo.y >= 0, 'the start-page logo is not clipped above the viewport')
  const enterWorkspace = compact.getByRole('button', { name: 'Continue without a project', exact: true })
  await enterWorkspace.scrollIntoViewIfNeeded()
  const continueBox = await enterWorkspace.boundingBox()
  // Browser scrolling rounds to whole pixels while the text layout is fractional.
  assert.ok(continueBox && continueBox.y >= -1 && continueBox.y + continueBox.height <= 701, 'the no-project entry stays reachable below a long create form')
  if (shots) await compact.screenshot({ path: join(shots, 'start-field-light.png') })
  await compact.close()

  assert.deepEqual(errors, [])
  console.log('PASS: name-only wizard, recent remove/delete, no-project entry, and minimum-size Field-mode reachability')
} finally {
  await browser.close()
}
