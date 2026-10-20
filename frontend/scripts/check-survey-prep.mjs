#!/usr/bin/env node
// Survey-area prepare failure classification (U9b).
//
//   node scripts/check-survey-prep.mjs
//
// The card shows a plain-language reason + next step for a failed prepare.
// The wording contract is the backend `dem-prep hrdem` messages: this checks
// the real classifier (`util/surveyPrep.ts`) against representative strings,
// including the ones dem-prep emits today, and the generic fallback.
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')

const out = join(await mkdtemp(join(tmpdir(), 'maggcs-prep-')), 'mod.mjs')
await build({
  entryPoints: [join(srcDir, 'util/surveyPrep.ts')],
  outfile: out,
  bundle: true,
  format: 'esm',
  platform: 'neutral',
  logLevel: 'silent',
})
const { classifyPrepError } = await import(`file://${out}`)
await rm(dirname(out), { recursive: true, force: true })

const failures = []
let passed = 0
function check(name, message, want) {
  const got = classifyPrepError(message)
  if (got === want) {
    passed += 1
    console.log(`  ok   ${name}`)
  } else {
    failures.push(name)
    console.log(`  FAIL ${name}: want ${want}, got ${got} (${JSON.stringify(message)})`)
  }
}

check('offline', 'dem-prep hrdem failed: offline or STAC unreachable: error sending request', 'offline')
check(
  'cross-project',
  'dem-prep hrdem failed: the AOI crosses project boundaries (2); choose a single project',
  'crossProject',
)
check('no coverage', 'dem-prep hrdem failed: no hrdem-lidar project covers the requested AOI', 'uncovered')
check(
  'oversized',
  'dem-prep hrdem failed: core AOI 150 m x 150 m exceeds the 100 m x 100 m survey-area limit',
  'oversized',
)
check('gdal missing (legacy wording)', 'dem-prep hrdem failed: ogr2ogr not found on PATH — install GDAL/PROJ', 'gdal')
check(
  'gdal missing (new wording)',
  'dem-prep hrdem failed: gdaltransform not found — dem-prep shells out to the GDAL/PROJ command-line tools (ADR-018). Install GDAL, or point MAGGCS_GDAL_PREFIX at a GDAL prefix',
  'gdal',
)
check(
  'installed gdal runtime failure is generic, not "not found"',
  'dem-prep hrdem failed: gdalwarp failed: ERROR 1: Cannot open source',
  'generic',
)
check('unknown wording', 'dem-prep hrdem failed: see console', 'generic')

console.log(`\ncheck:survey-prep ${failures.length === 0 ? 'OK' : 'FAILED'} — ${passed} passed, ${failures.length} failed`)
if (failures.length > 0) process.exit(1)
