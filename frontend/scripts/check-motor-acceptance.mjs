// Reproducible owned Motor browser run with retained logs/screenshots/hashes.
import { spawn, execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const frontend = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const root = dirname(frontend)
const port = Number(process.env.AF02B_MOTOR_PORT ?? 4182)
const url = `http://127.0.0.1:${port}/actuator.html`
const shots = join(frontend, 'screenshots/motor-test-owned')
const log = '/tmp/opencode/af02b-motor-browser-motor.log'
const recordPath = join(root, 'docs/verification/af-02b-motor-browser-20261027.json')
const hash = (value) => createHash('sha256').update(value).digest('hex')
await mkdir(shots, { recursive: true })
const vite = spawn(process.execPath, [join(frontend, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'], {
  cwd: frontend, stdio: ['ignore', 'pipe', 'pipe'],
})
let viteOutput = ''
vite.stdout.on('data', (bytes) => { viteOutput += bytes })
vite.stderr.on('data', (bytes) => { viteOutput += bytes })
async function screenshots(path) {
  const files = []
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const file = join(path, entry.name)
    if (entry.isDirectory()) files.push(...await screenshots(file))
    else if (entry.name.endsWith('.png')) {
      const bytes = await readFile(file)
      files.push({ path: relative(root, file), bytes: bytes.length, sha256: hash(bytes) })
    }
  }
  return files.sort((a, b) => a.path.localeCompare(b.path))
}
try {
  const deadline = Date.now() + 30000
  for (;;) {
    try { if ((await fetch(url)).ok) break } catch { /* wait */ }
    if (Date.now() >= deadline || vite.exitCode !== null) throw new Error(`Vite startup failed: ${viteOutput}`)
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  const started = Date.now()
  const child = spawn(process.execPath, [join(frontend, 'scripts/check-motor-controls.mjs'), url, shots], {
    cwd: frontend, stdio: ['ignore', 'pipe', 'pipe'],
  })
  let output = ''
  child.stdout.on('data', (bytes) => { output += bytes })
  child.stderr.on('data', (bytes) => { output += bytes })
  const exitCode = await new Promise((resolve, reject) => { child.once('error', reject); child.once('close', resolve) })
  await writeFile(log, output)
  const passed = output.match(/^PASS:/gm)?.length ?? 0
  const failed = output.match(/^FAIL:/gm)?.length ?? 0
  const record = {
    record_date: '2026-10-27', generated_at: new Date().toISOString(),
    source_base: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim(),
    source_includes_uncommitted_implementation: true,
    scope: 'AF-02b.4 owned Motor browser acceptance', result: exitCode === 0 ? 'pass' : 'fail',
    checks: [{ name: 'motor', command: ['node', 'scripts/check-motor-controls.mjs', url, relative(frontend, shots)],
      exit_code: exitCode, passed, failed, elapsed_ms: Date.now() - started, log, log_sha256: hash(output) }],
    screenshots: await screenshots(shots),
  }
  await writeFile(recordPath, `${JSON.stringify(record, null, 2)}\n`)
  console.log(`${exitCode === 0 ? 'PASS' : 'FAIL'} motor: ${passed} scenario groups, ${record.screenshots.length} screenshots; log ${log}`)
  if (exitCode !== 0) { console.error(output); process.exitCode = 1 }
} finally {
  vite.kill('SIGTERM')
  await writeFile('/tmp/opencode/af02b-motor-browser-vite.log', viteOutput)
}
