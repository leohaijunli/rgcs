// Run both ownership browser campaigns and retain one G1 manifest.
import { spawn } from 'node:child_process'
import { readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const frontend = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const root = dirname(frontend)
async function run(script) {
  const child = spawn('npm', ['run', script], { cwd: frontend, stdio: 'inherit' })
  const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('close', resolve) })
  if (code !== 0) throw new Error(`${script} failed`)
}
await run('check:shell-logs-acceptance')
await run('check:motor-acceptance')
const shell = JSON.parse(await readFile(join(root, 'docs/verification/af-02b-shell-logs-browser-20261026.json'), 'utf8'))
const motor = JSON.parse(await readFile(join(root, 'docs/verification/af-02b-motor-browser-20261027.json'), 'utf8'))
if (shell.result !== 'pass' || motor.result !== 'pass') throw new Error('ownership browser evidence failed')
const record = {
  record_date: '2026-10-27', generated_at: new Date().toISOString(),
  source_base: shell.source_base, source_includes_uncommitted_implementation: true,
  scope: 'AF-02 G1 cross-adapter browser qualification', result: 'pass',
  checks: [...shell.checks, ...motor.checks],
  screenshots: [...shell.screenshots, ...motor.screenshots]
    .sort((a, b) => a.path.localeCompare(b.path)),
}
await writeFile(join(root, 'docs/verification/af-02b-g1-browser-20261027.json'), `${JSON.stringify(record, null, 2)}\n`)
console.log(`PASS G1 browser: ${record.checks.length} checks, ${record.checks.reduce((sum, check) => sum + check.passed, 0)} assertions/groups, ${record.screenshots.length} screenshots`)
