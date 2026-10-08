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
const { compileWaypoints, commandUsesCoordinate, frameToAmsl, waypointFromItem } = await bundle(
  'mission/compile.ts',
)
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

console.log('command items (issues.md #34):')

check('only coordinate commands carry a global frame', () => {
  for (const command of [16, 21, 22, 17, 19, 82, 84, 85, 5001, 5100]) {
    assert(commandUsesCoordinate(command), `${command} carries a point`)
  }
  for (const command of [20, 93, 112, 178, 300, 401]) {
    assert(!commandUsesCoordinate(command), `${command} is a command item`)
  }
})

check('a DO_CHANGE_SPEED compiles to MAV_FRAME_MISSION in every mode', () => {
  const speed = { ...planned(48.6493, -123.3982, 130), command: 178 }
  for (const mode of ['relative', 'amsl', 'agl']) {
    const items = compileWaypoints([speed, ...plan], mode, home)
    eq(items[0].frame, 'mission', `frame in ${mode}`)
    eq(items[0].z, 130, `z is not re-datumed in ${mode}`)
    eq(items[1].frame, compileWaypoints(plan, mode, home)[0].frame, `waypoint frame in ${mode}`)
  }
})

check('a command item round-trips through the model unchanged', () => {
  const speed = { ...planned(0, 0, 0), command: 178, params: [1, 5, -1, 0] }
  const item = compileWaypoints([speed], 'relative', home)[0]
  const back = waypointFromItem(item, home)
  eq(back.command, 178, 'command')
  eq(back.altitude.meters, 0, 'z is untouched by the datum')
  eq(back.params[1], 5, 'params')
})

check('a QGC command item without a coordinate is imported, not dropped', () => {
  const plan = {
    fileType: 'Plan',
    version: 1,
    mission: {
      items: [
        {
          autoContinue: true,
          command: 178,
          doJumpId: 1,
          frame: 2,
          params: [1, 5, -1, 0, 0, 0, 0],
          type: 'SimpleItem',
        },
      ],
    },
  }
  const imp = parsePlan(JSON.stringify(plan))
  eq(imp.items.length, 1, 'imported')
  eq(imp.items[0].frame, 'mission', 'frame')
  eq(imp.items[0].params[1], 5, 'speed kept')
  eq(imp.unsupported.length, 0, 'nothing skipped')
})

check('a command item exports as MAV_FRAME_MISSION', () => {
  const items = compileWaypoints([{ ...planned(0, 0, 0), command: 178 }], 'relative', home)
  const out = JSON.parse(buildPlan(items, 'relative'))
  eq(out.mission.items[0].frame, 2, 'frame 2 in the .plan')
})

console.log('pattern line colouring (issues.md #35):')

const { kindsBySeq, splitRuns, heightRuns } = await bundle('mission/lineKinds.ts')

check('the line table maps seq ranges to kinds', () => {
  const kinds = kindsBySeq([
    { id: 1, kind: 'survey', start_seq: 0, end_seq: 2, length_m: 10 },
    { id: 1, kind: 'tie', start_seq: 3, end_seq: 3, length_m: 10 },
    { id: 2, kind: 'survey', start_seq: 4, end_seq: 5, length_m: 10 },
  ])
  eq(kinds.get(0), 'survey', 'seq 0')
  eq(kinds.get(2), 'survey', 'seq 2')
  eq(kinds.get(3), 'tie', 'seq 3')
  eq(kinds.get(4), 'survey', 'seq 4')
  eq(kinds.get(9), undefined, 'outside the table')
})

check('runs never merge different kinds', () => {
  const items = [0, 1, 2, 3, 4, 5].map((seq) => ({ seq }))
  const kinds = kindsBySeq([
    { id: 1, kind: 'survey', start_seq: 0, end_seq: 2, length_m: 1 },
    { id: 1, kind: 'tie', start_seq: 3, end_seq: 3, length_m: 1 },
    { id: 2, kind: 'survey', start_seq: 4, end_seq: 5, length_m: 1 },
  ])
  const runs = splitRuns(items, kinds)
  eq(runs.length, 3, 'three runs')
  eq(runs.map((r) => r.kind).join(','), 'survey,tie,survey', 'kinds in order')
  eq(runs.map((r) => r.items.length).join(','), '3,1,2', 'sizes')
})

check('unclassified waypoints form their own run', () => {
  const items = [0, 1, 2].map((seq) => ({ seq }))
  const runs = splitRuns(items, kindsBySeq([]))
  eq(runs.length, 1, 'one run')
  eq(runs[0].kind, null, 'no kind')
})

check('without a ground anchor heights use the lowest waypoint', () => {
  const runs = heightRuns([
    { seq: 0, z: 130 },
    { seq: 1, z: 145 },
    { seq: 2, z: 130 },
  ])
  eq(runs[0].base, 130, 'base')
  eq(runs[1].top - runs[1].base, 15, 'climb')
  eq(runs[2].top - runs[2].base, 0, 'level waypoint')
})

check('a ground anchor stands every stick on the ground (issues.md #36)', () => {
  const runs = heightRuns([{ seq: 0, z: 130 }, { seq: 1, z: 145 }], 100)
  eq(runs[0].base, 100, 'base')
  eq(runs[0].top, 130, 'top')
  eq(runs[0].top - runs[0].base, 30, 'clearance of the lower waypoint')
  eq(runs[1].top - runs[1].base, 45, 'clearance of the higher waypoint')
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
