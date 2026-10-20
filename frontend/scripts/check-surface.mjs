#!/usr/bin/env node
// DEM surface resolution order (drape button / altitude profile).
//
//   node scripts/check-surface.mjs
//
// `firstSurface` (util/surface.ts) is the pure core of `ensureTile`: it must
// return the first source that yields a tile path, fall through sources that
// return null or throw, stop once a path is found, and never throw itself.
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')

const out = join(await mkdtemp(join(tmpdir(), 'maggcs-surface-')), 'mod.mjs')
await build({
  entryPoints: [join(srcDir, 'util/surface.ts')],
  outfile: out,
  bundle: true,
  format: 'esm',
  platform: 'neutral',
  logLevel: 'silent',
})
const { firstSurface } = await import(`file://${out}`)
await rm(dirname(out), { recursive: true, force: true })

const failures = []
let passed = 0

async function check(name, want, ...sources) {
  let got
  try {
    got = await firstSurface(...sources)
  } catch (e) {
    failures.push(name)
    console.log(`  FAIL ${name}: threw ${String(e)}`)
    return
  }
  if (got === want) {
    passed += 1
    console.log(`  ok   ${name}`)
  } else {
    failures.push(name)
    console.log(`  FAIL ${name}: want ${JSON.stringify(want)}, got ${JSON.stringify(got)}`)
  }
}

const src = (path) => async () => path

await check('first source wins', '/a', src('/a'), src('/b'), src('/c'))
await check('no source yields a path', null)
await check('null sources fall through to the last', '/c', src(null), src(null), src('/c'))
await check('throwing sources fall through', '/b', async () => {
  throw new Error('no session yet')
}, src('/b'))
await check('all throw resolves to null', null, async () => {
  throw new Error('remembered folder gone')
}, async () => {
  throw new Error('GDAL missing')
})

{
  const ran = []
  const result = await firstSurface(
    async () => {
      ran.push('slow')
      return null
    },
    async () => {
      ran.push('hit')
      return '/only'
    },
    async () => {
      ran.push('never')
      return '/late'
    },
  )
  if (result === '/only' && !ran.includes('never') && ran.includes('hit')) {
    passed += 1
    console.log('  ok   stops after the first hit (later sources do not run)')
  } else {
    failures.push('stop-after-hit')
    console.log(`  FAIL stop-after-hit: ran=${JSON.stringify(ran)} result=${result}`)
  }
}

console.log(`\ncheck:surface ${failures.length === 0 ? 'OK' : 'FAILED'} — ${passed} passed, ${failures.length} failed`)
if (failures.length > 0) process.exit(1)
