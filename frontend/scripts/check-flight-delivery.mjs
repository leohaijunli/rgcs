// AF-02b.4b: run the final workspace/build gates and retain measured evidence.
// Browser acceptance is a separate command, so passing browser checks are not
// repeated by a native build. Run from frontend/ or via npm --prefix frontend.
import { spawn, execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const frontend = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const root = dirname(frontend)
const missionMode = process.argv.includes('--mission')
const shellLogsMode = process.argv.includes('--shell-logs')
const motorMode = process.argv.includes('--motor')
const g1Mode = process.argv.includes('--g1')
const scope = g1Mode ? 'af-02b-g1' : motorMode ? 'af-02b-motor' : shellLogsMode ? 'af-02b-shell-logs' : missionMode ? 'af-02b-mission' : 'af-02b-flight-command'
const evidenceDate = motorMode || g1Mode ? '20261027' : '20261026'
const evidencePath = join(root, `docs/verification/${scope}-build-${evidenceDate}.json`)
const browserPath = join(root, `docs/verification/${scope}-browser-${evidenceDate}.json`)
const logPrefix = g1Mode ? 'af02b-g1' : motorMode ? 'af02b-motor' : shellLogsMode ? 'af02b-shell-logs' : missionMode ? 'af02b-mission' : 'af02b4b'
const environment = { ...process.env, PATH: `${join(homedir(), '.cargo/bin')}:${process.env.PATH ?? ''}` }
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex')
const CHECK_TIMEOUT_MS = 600_000
const specs = [
  ['frontend-build', 'npm', ['run', 'build'], frontend],
  ['dem-helper', 'cargo', ['build', '-p', 'dem-prep', '--bin', 'dem-prep'], root],
  ['rust-all-targets', 'cargo', ['test', '--workspace', '--all-targets'], root],
  ['rust-doc-tests', 'cargo', ['test', '--workspace', '--doc'], root],
  ['clippy', 'cargo', ['clippy', '--workspace', '--all-targets', '--', '-D', 'warnings'], root],
  ['format', 'cargo', ['fmt', '--all', '--', '--check'], root],
  ...['generated-types', 'i18n', 'colors', 'contrast', 'planfile', 'mission-sync', 'undo', 'survey-prep', 'surface', 'plan-steps', 'coords', 'agl', 'progress', 'polygon', 'drape-error', 'plan-profile', 'motor-scope'].map((name) => [`frontend-${name}`, 'npm', ['run', `check:${name}`], frontend]),
  ['stream-catalog', 'python3', ['scripts/generate-stream-catalog.py', '--check'], root],
  ['whitespace', 'git', ['diff', '--check'], root],
  ['release', 'cargo', ['build', '--release', '--workspace'], root],
]

async function sourceFingerprint() {
  const paths = execFileSync('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard', '--',
    '.gitattributes', '.github/workflows',
    'Cargo.toml', 'Cargo.lock', 'crates/core/Cargo.toml', 'crates/core/src', 'crates/core/tests',
    'crates/app-tauri/Cargo.toml', 'crates/app-tauri/src', 'crates/app-tauri/tests', 'crates/app-tauri/tauri.conf.json',
    'tools/dem-prep/Cargo.toml', 'tools/dem-prep/src', 'frontend/src', 'frontend/scripts',
    'frontend/package.json', 'frontend/package-lock.json', 'frontend/vite.config.ts'], { cwd: root, encoding: 'utf8' })
    .split('\0').filter(Boolean)
  const manifest = []
  for (const path of [...new Set(paths)].sort()) manifest.push(`${path}\0${hash(await readFile(join(root, path)))}`)
  return { file_count: manifest.length, sha256: hash(manifest.join('\n')) }
}

async function runCheck([name, command, args, cwd]) {
  const started = Date.now()
  const child = spawn(command, args, { cwd, env: environment, stdio: ['ignore', 'pipe', 'pipe'] })
  let output = ''
  child.stdout.on('data', (bytes) => { output += bytes })
  child.stderr.on('data', (bytes) => { output += bytes })
  const timeout = setTimeout(() => child.kill('SIGTERM'), CHECK_TIMEOUT_MS)
  const exitCode = await new Promise((resolve, reject) => {
    child.once('error', reject)
    child.once('close', resolve)
  }).finally(() => clearTimeout(timeout))
  const log = `/tmp/opencode/${logPrefix}-build-${name}.log`
  await writeFile(log, output)
  const summaries = [...output.matchAll(/test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored/g)]
    .map((match) => ({ result: match[1], passed: Number(match[2]), failed: Number(match[3]), ignored: Number(match[4]) }))
  const result = {
    name, command: [command, ...args], cwd: relative(root, cwd) || '.',
    exit_code: exitCode, elapsed_ms: Date.now() - started, log, log_sha256: hash(output),
    ...(summaries.length > 0 ? { test_summaries: summaries, total_passed: summaries.reduce((sum, item) => sum + item.passed, 0), total_ignored: summaries.reduce((sum, item) => sum + item.ignored, 0) } : {}),
    ...(name === 'frontend-build' ? { modules_transformed: Number(output.match(/(\d+) modules transformed/)?.[1] ?? 0) } : {}),
  }
  console.log(`${exitCode === 0 ? 'PASS' : 'FAIL'} ${name} (${result.elapsed_ms} ms)${summaries.length > 0 ? `: ${result.total_passed} tests passed, ${result.total_ignored} ignored` : ''}; log ${log}`)
  if (exitCode !== 0) console.error(output)
  return result
}

async function artifact(path) {
  const bytes = await readFile(join(root, path))
  return { path, bytes: bytes.length, sha256: hash(bytes) }
}

async function frontendArtifact(directory) {
  const manifest = []
  let bytes = 0
  async function walk(path) {
    for (const entry of await readdir(path, { withFileTypes: true })) {
      const file = join(path, entry.name)
      if (entry.isDirectory()) await walk(file)
      else {
        const content = await readFile(file)
        bytes += content.length
        manifest.push(`${relative(directory, file)}\0${hash(content)}`)
      }
    }
  }
  await walk(directory)
  return { path: relative(root, directory), file_count: manifest.length, bytes, manifest_sha256: hash(manifest.sort().join('\n')) }
}

const browserBytes = await readFile(browserPath)
await mkdir('/tmp/opencode', { recursive: true })
const browser = JSON.parse(browserBytes)
const requiredBrowserChecks = g1Mode
  ? ['shell_logs', 'mission', 'ux', 'safety', 'preflight', 'topbar', 'teardown', 'startup', 'heartbeat', 'motor']
  : motorMode ? ['motor'] : [...(shellLogsMode ? ['shell_logs'] : []), ...(missionMode || shellLogsMode ? ['mission'] : []), 'ux', 'safety', 'preflight', 'topbar', 'teardown', 'startup', 'heartbeat']
if (browser.result !== 'pass' || requiredBrowserChecks.some((name) => !browser.checks.some((check) => check.name === name && check.exit_code === 0))) {
  throw new Error('Run the complete check:flight-acceptance suite before recording final delivery')
}
for (const shot of browser.screenshots) {
  if (hash(await readFile(join(root, shot.path))) !== shot.sha256) throw new Error(`Screenshot evidence changed: ${shot.path}`)
}

if (process.argv.includes('--verify-evidence')) {
  const retained = JSON.parse(await readFile(evidencePath, 'utf8'))
  if (retained.result !== 'pass' || retained.checks.length !== specs.length
    || retained.source_before.sha256 !== retained.source_after.sha256
    || retained.browser_evidence.sha256 !== hash(browserBytes)) {
    throw new Error('The retained final delivery record is incomplete or inconsistent')
  }
  for (const [name, command, args] of specs) {
    const check = retained.checks.find((candidate) => candidate.name === name)
    if (!check || check.exit_code !== 0 || JSON.stringify(check.command) !== JSON.stringify([command, ...args])
      || hash(await readFile(check.log)) !== check.log_sha256) {
      throw new Error(`Delivery log/command evidence changed: ${name}`)
    }
  }
  for (const check of browser.checks) {
    if (hash(await readFile(check.log)) !== check.log_sha256) throw new Error(`Browser log evidence changed: ${check.name}`)
  }
  const actualArtifacts = {
    frontend: await frontendArtifact(join(frontend, 'dist')),
    desktop: await artifact('target/release/maggcs-app'),
    dem_helper: await artifact('target/release/dem-prep'),
  }
  if (JSON.stringify(actualArtifacts) !== JSON.stringify(retained.artifacts)) {
    throw new Error('The retained production/release artifacts changed')
  }
  const currentSource = await sourceFingerprint()
  if (currentSource.sha256 !== retained.source_after.sha256) throw new Error('Source inputs changed after the retained verification')
  console.log(`PASS final evidence: ${retained.checks.length} gates, ${browser.checks.length} browser checks, ${browser.screenshots.length} screenshots and 3 artifact manifests match`)
  process.exit(0)
}

const sourceBefore = await sourceFingerprint()
const record = {
  record_date: '2026-10-26', generated_at: new Date().toISOString(),
  source_base: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim(),
  source_includes_uncommitted_implementation: true,
  scope: g1Mode ? 'AF-02 G1 final cross-adapter ownership qualification' : motorMode ? 'AF-02b.4 Motor final workspace/protocol/static/production/release gates' : shellLogsMode ? 'AF-02b.4 Shell/Logs final workspace/protocol/static/production/release gates' : missionMode ? 'AF-02b.4 Mission final workspace/protocol/static/production/release gates' : 'AF-02b.4b final workspace/protocol/static/production/release gates',
  reproduction: ['npm', '--prefix', 'frontend', 'run', g1Mode ? 'check:g1-delivery' : motorMode ? 'check:motor-delivery' : shellLogsMode ? 'check:shell-logs-delivery' : missionMode ? 'check:mission-delivery' : 'check:flight-delivery'],
  source_before: sourceBefore, result: 'in_progress', checks: [],
  browser_evidence: { path: relative(root, browserPath), sha256: hash(browserBytes), checks: browser.checks.length, passed: browser.checks.reduce((sum, check) => sum + check.passed, 0), screenshots: browser.screenshots.length },
}

try {
  for (const spec of specs) {
    const result = await runCheck(spec)
    record.checks.push(result)
    await writeFile(evidencePath, `${JSON.stringify(record, null, 2)}\n`)
    if (result.exit_code !== 0) throw new Error(`Delivery check failed: ${result.name}`)
  }
  record.source_after = await sourceFingerprint()
  if (record.source_after.sha256 !== sourceBefore.sha256) throw new Error('Source inputs changed during verification')
  record.artifacts = {
    frontend: await frontendArtifact(join(frontend, 'dist')),
    desktop: await artifact('target/release/maggcs-app'),
    dem_helper: await artifact('target/release/dem-prep'),
  }
  record.result = 'pass'
} catch (error) {
  record.result = 'fail'
  record.error = String(error)
  process.exitCode = 1
} finally {
  record.completed_at = new Date().toISOString()
  await writeFile(evidencePath, `${JSON.stringify(record, null, 2)}\n`)
  console.log(`Evidence: ${relative(root, evidencePath)} (${record.result})`)
}
