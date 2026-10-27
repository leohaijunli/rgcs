// AF-02b.4b reproducible browser acceptance. Own one loopback Vite process,
// retain each check's full log and screenshots, and write measured JSON evidence.
// Usage: node scripts/check-flight-acceptance.mjs [ux,safety,preflight,...]
import { spawn, execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const frontend = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const root = dirname(frontend)
const missionMode = process.argv.includes('--mission')
const port = Number(process.env.AF02B4B_PORT ?? (missionMode ? 4180 : 4179))
const url = `http://127.0.0.1:${port}/`
const shotsRoot = join(frontend, missionMode ? 'screenshots/mission/ownership' : 'screenshots/commands/ownership')
const logRoot = '/tmp/opencode'
const recordPath = join(root, `docs/verification/${missionMode ? 'af-02b-mission' : 'af-02b-flight-command'}-browser-20261026.json`)
const logPrefix = missionMode ? 'af02b-mission' : 'af02b4b'
const scripts = {
  ...(missionMode ? { mission: 'check-mission-ownership.mjs' } : {}),
  ux: 'check-command-ux.mjs',
  safety: 'check-safety.mjs',
  preflight: 'check-preflight.mjs',
  topbar: 'check-topbar.mjs',
  teardown: 'check-teardown.mjs',
  startup: 'check-startup.mjs',
  heartbeat: 'check-heartbeat.mjs',
}
const selected = process.argv.slice(2).find((argument) => argument !== '--mission')?.split(',') ?? Object.keys(scripts)
if (selected.some((name) => !scripts[name])) throw new Error('Unknown browser check')
const CHECK_TIMEOUT_MS = 180_000
const START_TIMEOUT_MS = 30_000
const MAX_PARALLEL_CHECKS = 3
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex')

await mkdir(shotsRoot, { recursive: true })
await mkdir(logRoot, { recursive: true })
const vite = spawn(process.execPath, [join(frontend, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'], {
  cwd: frontend,
  stdio: ['ignore', 'pipe', 'pipe'],
})
let viteOutput = ''
vite.stdout.on('data', (bytes) => { viteOutput += bytes })
vite.stderr.on('data', (bytes) => { viteOutput += bytes })
vite.on('error', (error) => { viteOutput += String(error) })
const results = []

async function runCheck(name) {
  const args = [join(frontend, 'scripts', scripts[name]), url, join(shotsRoot, name)]
  const started = Date.now()
  const child = spawn(process.execPath, args, { cwd: frontend, stdio: ['ignore', 'pipe', 'pipe'] })
  let output = ''
  child.stdout.on('data', (bytes) => { output += bytes })
  child.stderr.on('data', (bytes) => { output += bytes })
  const timeout = setTimeout(() => child.kill('SIGTERM'), CHECK_TIMEOUT_MS)
  const code = await new Promise((resolve, reject) => {
    child.once('error', reject)
    child.once('close', resolve)
  }).finally(() => clearTimeout(timeout))
  const log = join(logRoot, `${logPrefix}-browser-${name}.log`)
  await writeFile(log, output)
  const passed = output.match(/^PASS /gm)?.length ?? 0
  const failed = output.match(/^FAIL /gm)?.length ?? 0
  console.log(`${code === 0 ? 'PASS' : 'FAIL'} ${name}: ${passed} assertions passed, ${failed} failed (${Date.now() - started} ms); log ${log}`)
  if (code !== 0) console.error(output)
  return { name, command: ['node', relative(frontend, args[0]), url, relative(frontend, args[2])], exit_code: code, passed, failed, elapsed_ms: Date.now() - started, log, log_sha256: hash(output) }
}

async function screenshots(directory) {
  const files = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) files.push(...await screenshots(path))
    else if (entry.name.endsWith('.png')) {
      const bytes = await readFile(path)
      files.push({ path: relative(root, path), bytes: bytes.length, sha256: hash(bytes) })
    }
  }
  return files.sort((a, b) => a.path.localeCompare(b.path))
}

try {
  const deadline = Date.now() + START_TIMEOUT_MS
  for (;;) {
    if (vite.exitCode !== null) throw new Error(`Vite stopped: ${viteOutput}`)
    try {
      if ((await fetch(url)).ok) break
    } catch { /* Wait for the owned server to bind. */ }
    if (Date.now() >= deadline) throw new Error(`Vite startup deadline: ${viteOutput}`)
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  for (let index = 0; index < selected.length; index += MAX_PARALLEL_CHECKS) {
    results.push(...await Promise.all(selected.slice(index, index + MAX_PARALLEL_CHECKS).map(runCheck)))
  }
  const record = {
    record_date: '2026-10-26',
    generated_at: new Date().toISOString(),
    source_base: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim(),
    source_includes_uncommitted_implementation: true,
    scope: missionMode ? 'AF-02b.4 owned Mission and related flight/shell browser regression' : 'AF-02b.4b browser flight-command ownership and related shell regression',
    reproduction: ['node', 'scripts/check-flight-acceptance.mjs', ...(missionMode ? ['--mission'] : []), selected.join(',')],
    result: results.every((result) => result.exit_code === 0) ? 'pass' : 'fail',
    checks: results,
    screenshots: await screenshots(shotsRoot),
  }
  await writeFile(recordPath, `${JSON.stringify(record, null, 2)}\n`)
  console.log(`Evidence: ${relative(root, recordPath)} (${record.screenshots.length} screenshots)`)
  if (record.result !== 'pass') process.exitCode = 1
} finally {
  vite.kill('SIGTERM')
  await writeFile(join(logRoot, `${logPrefix}-browser-vite.log`), viteOutput)
}
