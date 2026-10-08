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
const { convertAltitudeZ } = await bundle('mission/altitude.ts')

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

console.log('altitude:')

check('relative <-> amsl conversion preserves AMSL (finding 1)', () => {
  const anchor = { homeAmslM: 488 }
  eq(convertAltitudeZ(30, 'relative', 'amsl', anchor), 518, 'relative -> amsl')
  eq(convertAltitudeZ(518, 'amsl', 'relative', anchor), 30, 'amsl -> relative')
})

check('agl converts via the home/ground anchor', () => {
  const anchor = { homeAmslM: 488 }
  eq(convertAltitudeZ(518, 'amsl', 'agl', anchor), 30, 'amsl -> agl')
  eq(convertAltitudeZ(30, 'agl', 'amsl', anchor), 518, 'agl -> amsl')
})

check('identical mode is a no-op', () => {
  eq(convertAltitudeZ(42, 'amsl', 'amsl', { homeAmslM: 488 }), 42, 'no-op')
})

if (failures.length > 0) {
  console.error(`\n${failures.length} check(s) failed`)
  process.exit(1)
}
console.log('\nall planfile checks passed')
