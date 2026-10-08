#!/usr/bin/env node
// `.plan` round-trip + altitude-datum checks (findings 1/3/4/5).
//
//   node scripts/check-planfile.mjs
//
// Bundles the pure mission modules with esbuild (stubbing the Tauri plugins,
// which have no runtime in Node) and asserts the import/export and datum
// conversions. Exits non-zero and prints the offender on failure.
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')
const repoRoot = join(here, '..', '..')

const tauriStub = {
  name: 'stub-tauri',
  setup(build) {
    build.onResolve({ filter: /^@tauri-apps\// }, (args) => ({
      path: args.path,
      namespace: 'tauri-stub',
    }))
    build.onLoad({ filter: /.*/, namespace: 'tauri-stub' }, () => ({
      contents:
        'export const readTextFile=async()=>"";export const writeTextFile=async()=>{};' +
        'export const open=async()=>null;export const save=async()=>null;export const invoke=async()=>{};',
      loader: 'js',
    }))
  },
}

async function bundle(entry) {
  const out = join(await mkdtemp(join(tmpdir(), 'maggcs-plan-')), 'mod.mjs')
  await build({
    entryPoints: [join(srcDir, entry)],
    outfile: out,
    bundle: true,
    format: 'esm',
    platform: 'neutral',
    plugins: [tauriStub],
    logLevel: 'silent',
  })
  const mod = await import(`file://${out}`)
  await rm(dirname(out), { recursive: true, force: true })
  return mod
}

const { parsePlan, buildPlan, orderedMissionItems } = await bundle('mission/planfile.ts')
const { itemsHash } = await bundle('mission/hash.ts')
const { compileWaypoints, frameToAmsl, waypointFromItem } = await bundle('mission/compile.ts')
const { rectanglePolygon, defaultSweep, defaultCloverleaf } = await bundle('mission/patterns.ts')

const failures = []
function check(name, fn) {
  try {
    fn()
    console.log(`  ok   ${name}`)
  } catch (e) {
    failures.push(`${name}: ${e.message}`)
    console.log(`  FAIL ${name}: ${e.message}`)
  }
}
function assert(cond, msg) {
  if (!cond) throw new Error(msg)
}
function eq(a, b, msg) {
  assert(a === b, `${msg ?? 'not equal'}: ${JSON.stringify(a)} !== ${JSON.stringify(b)}`)
}

const fixture = JSON.parse(await readFile(join(repoRoot, 'testdata', 'qgc-survey.plan'), 'utf8'))
const surveyRaw = fixture.mission.items[1]

console.log('planfile:')

check('complex items are kept as blocks, not flattened (finding 3)', () => {
  const imp = parsePlan(JSON.stringify(fixture))
  eq(imp.items.length, 1, 'simple items')
  eq(imp.blocks.length, 1, 'blocks')
  eq(imp.blocks[0].type, 'Survey', 'block type')
  eq(imp.blocks[0].children.length, 2, 'survey children')
  eq(imp.mode, 'relative', 'altitude mode')
})

check('child seq does not collide with editable item seq', () => {
  const imp = parsePlan(JSON.stringify(fixture))
  const seqs = new Set(imp.items.map((i) => i.seq))
  imp.blocks[0].children.forEach((c) => assert(!seqs.has(c.seq), `child seq ${c.seq} collides`))
})

check('flyable order mirrors the source file (finding 3)', () => {
  const imp = parsePlan(JSON.stringify(fixture))
  const order = orderedMissionItems(imp.items, imp.blocks, imp.base)
  eq(order.length, 3, 'flyable count')
  eq(order[0].command, 16, 'first is the simple item')
  eq(order[1].seq, imp.blocks[0].children[0].seq, 'second is the first survey child')
  eq(order[2].seq, imp.blocks[0].children[1].seq, 'third is the second survey child')
})

check('round trip preserves the survey verbatim (finding 3)', () => {
  const imp = parsePlan(JSON.stringify(fixture))
  const out = JSON.parse(buildPlan(imp.items, imp.mode, { base: imp.base, home: imp.home, blocks: imp.blocks }))
  const re = parsePlan(JSON.stringify(out))
  eq(re.items.length, 1, 'simple items survive')
  eq(re.blocks.length, 1, 'survey block survives')
  eq(re.blocks[0].type, 'Survey', 'survey type')
  eq(JSON.stringify(re.blocks[0].raw), JSON.stringify(surveyRaw), 'survey JSON is unchanged')
  eq(re.blocks[0].raw.gridSpacing, 20, 'gridSpacing kept')
  eq(re.blocks[0].raw.polygon.length, 3, 'polygon kept')
  eq(re.items[0].command, imp.items[0].command, 'command kept')
  eq(re.items[0].x, imp.items[0].x, 'lat kept')
  eq(re.items[0].z, imp.items[0].z, 'alt kept')
  eq(re.items[0].frame, imp.items[0].frame, 'frame kept')
})

check('doJumpId survives an unedited round trip', () => {
  const imp = parsePlan(JSON.stringify(fixture))
  const out = JSON.parse(buildPlan(imp.items, imp.mode, { base: imp.base, home: imp.home, blocks: imp.blocks }))
  eq(out.mission.items[0].doJumpId, fixture.mission.items[0].doJumpId, 'simple doJumpId')
  eq(out.mission.items[1].doJumpId, surveyRaw.doJumpId, 'survey doJumpId')
})

check('null params are coerced to finite numbers (finding 4)', () => {
  const plan = {
    fileType: 'Plan',
    version: 1,
    mission: {
      items: [
        {
          command: 16,
          frame: 6,
          doJumpId: 1,
          params: [null, 2, null, null, 47.4, 8.5, 30],
          type: 'SimpleItem',
          coordinate: [47.4, 8.5, 30],
        },
      ],
    },
  }
  const imp = parsePlan(JSON.stringify(plan))
  eq(imp.items.length, 1, 'item parsed')
  imp.items[0].params.forEach((p, i) => assert(Number.isFinite(p), `param ${i} is ${p}`))
})

check('itemsHash ignores seq and current (finding 5)', () => {
  const a = [{ seq: 0, frame: 'global_relative_alt_int', command: 16, params: [0, 0, 0, 0], x: 1, y: 2, z: 3, autocontinue: true, current: true }]
  const b = [{ seq: 7, frame: 'global_relative_alt_int', command: 16, params: [0, 0, 0, 0], x: 1, y: 2, z: 3, autocontinue: true, current: false }]
  eq(itemsHash(a), itemsHash(b), 'hash must not depend on seq/current')
  const c = [{ ...b[0], z: 4 }]
  assert(itemsHash(a) !== itemsHash(c), 'hash must still detect a real change')
})

console.log('compile (ADR-013):')

function planned(lat, lon, amslM) {
  return {
    position: { latitude_deg: lat, longitude_deg: lon },
    altitude: { datum: 'AMSL_EGM96', meters: amslM },
    command: 16,
    params: [0, 0, 0, 0],
    autocontinue: true,
  }
}

const plan = [planned(48.6493, -123.3982, 130), planned(48.65, -123.397, 180)]
const home = 100

check('absolute and relative compiles share geometry (finding 1)', () => {
  const abs = compileWaypoints(plan, 'amsl', home)
  const rel = compileWaypoints(plan, 'relative', home)
  eq(abs[0].frame, 'global_int', 'amsl frame')
  eq(rel[0].frame, 'global_relative_alt_int', 'relative frame')
  abs.forEach((a, i) => {
    eq(a.x, rel[i].x, `lat ${i}`)
    eq(a.y, rel[i].y, `lon ${i}`)
    eq(a.z, rel[i].z + home, `z offset ${i}`)
  })
  eq(abs[0].z, 130, 'absolute z is AMSL')
  eq(rel[0].z, 30, 'relative z is AMSL - home')
})

check('agl compiles to the terrain frame with the flat-ground anchor', () => {
  const agl = compileWaypoints(plan, 'agl', home)
  eq(agl[0].frame, 'global_terrain_alt_int', 'terrain frame')
  eq(agl[0].z, 30, 'z = AMSL - home ground')
})

check('wire item round-trips back to AMSL', () => {
  const rel = compileWaypoints(plan, 'relative', home)
  const wp = waypointFromItem(rel[1], home)
  eq(wp.position.latitude_deg, plan[1].position.latitude_deg, 'lat')
  eq(wp.altitude.datum, 'AMSL_EGM96', 'datum')
  eq(wp.altitude.meters, 180, 'AMSL restored')
})

check('frameToAmsl is the inverse of the compile offset', () => {
  eq(frameToAmsl(30, 'global_relative_alt_int', 100), 130, 'relative -> AMSL')
  eq(frameToAmsl(130, 'global_int', 100), 130, 'absolute is identity')
})

console.log('patterns (TS side):')

check('rectangle preset spans the requested size', () => {
  const center = { latitude_deg: 48, longitude_deg: -123 }
  const r = rectanglePolygon(center, 1000, 600)
  eq(r.length, 4, 'four corners')
  const R = 6378137
  const d2r = Math.PI / 180
  const eastSpan = (r[1].longitude_deg - r[0].longitude_deg) * d2r * R * Math.cos(48 * d2r)
  const northSpan = (r[3].latitude_deg - r[0].latitude_deg) * d2r * R
  assert(Math.abs(eastSpan - 1000) < 0.01, `east span ${eastSpan}`)
  assert(Math.abs(northSpan - 600) < 0.01, `north span ${northSpan}`)
})

check('pattern defaults are usable', () => {
  const center = { latitude_deg: 48, longitude_deg: -123 }
  const sweep = defaultSweep(center, 150)
  eq(sweep.polygon.length, 4, 'sweep polygon')
  assert(sweep.line_spacing_m > 0, 'spacing')
  eq(sweep.altitude_amsl_m, 150, 'sweep altitude')
  const clover = defaultCloverleaf(center, 150)
  eq(clover.petals, 4, 'petals')
  eq(clover.center.latitude_deg, 48, 'centre')
})

if (failures.length > 0) {
  console.error(`\n${failures.length} check(s) failed`)
  process.exit(1)
}
console.log('\nall planfile checks passed')
