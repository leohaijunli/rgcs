#!/usr/bin/env node
// DEM drape failure messages (ADR-004 fail-closed).
//
//   node scripts/check-drape-error.mjs
//
// The core terrain reader fails closed on the first waypoint outside the DEM
// or on a void. `drapeErrorReason` (util/drapeError.ts) must fold those terse
// backend strings into one actionable message and let anything else through
// unchanged, so the planning panel never shows a bare "point outside the DEM
// coverage".
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')

const out = join(await mkdtemp(join(tmpdir(), 'maggcs-drape-error-')), 'mod.mjs')
await build({
  entryPoints: [join(srcDir, 'util/drapeError.ts')],
  outfile: out,
  bundle: true,
  format: 'esm',
  platform: 'neutral',
  logLevel: 'silent',
})
const { drapeErrorReason, DEM_NOT_COVERED_ERROR } = await import(`file://${out}`)
await rm(dirname(out), { recursive: true, force: true })

const failures = []
let passed = 0

function check(name, want, raw) {
  const got = drapeErrorReason(raw)
  if (got === want) {
    passed += 1
    console.log(`  ok   ${name}`)
  } else {
    failures.push(name)
    console.log(`  FAIL ${name}: want ${JSON.stringify(want)}, got ${JSON.stringify(got)}`)
  }
}

check('outside coverage folds to the actionable message', DEM_NOT_COVERED_ERROR, 'point outside the DEM coverage')
check('DEM void folds to the same message', DEM_NOT_COVERED_ERROR, 'DEM void')
check('case-insensitive', DEM_NOT_COVERED_ERROR, 'POINT OUTSIDE THE DEM COVERAGE')
check('unrelated errors pass through', 'network timeout', 'network timeout')
check('empty string passes through', '', '')

if (failures.length > 0) {
  console.log(`\n${failures.length} failure(s)`)
  process.exit(1)
}
console.log(`\n${passed} passed`)
