#!/usr/bin/env node
// AGL ground-reference checks (operator bug: the drone's map readout showed the
// AMSL altitude labelled "AGL" whenever the plan carried no HOME elevation).
//
//   node scripts/check-agl.mjs
//
// Pure module, bundled with esbuild like the other checks: asserts the ground
// the vehicle is measured against, and that a hovering/landed vehicle reads a
// real height above ground rather than echoing its absolute altitude.
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')

/** Bundle a pure src module for direct assertions. */
async function load(entry) {
  const out = join(await mkdtemp(join(tmpdir(), 'maggcs-agl-')), 'mod.mjs')
  await build({
    entryPoints: [join(srcDir, ...entry.split('/'))],
    outfile: out,
    bundle: true,
    format: 'esm',
    platform: 'neutral',
    logLevel: 'silent',
  })
  const mod = await import(`file://${out}`)
  await rm(dirname(out), { recursive: true, force: true })
  return mod
}

const { groundReferenceAmsl, heightAboveGroundM } = await load('cesium/heights.ts')
// Terrain drape against the live 3D mesh (operator bug: the drape reported the
// requested 40 m AGL as an absolute 40 m AMSL whenever the mesh was the only
// surface loaded). Same contract as the backend drape.
const { drapeToMesh } = await load('mission/meshDrape.ts')

const failures = []
let passed = 0
function check(name, fn) {
  try {
    fn()
    passed += 1
    console.log(`  ok   ${name}`)
  } catch (e) {
    failures.push(`${name}: ${e.message}`)
    console.log(`  FAIL ${name}: ${e.message}`)
  }
}
function assert(cond, msg) {
  if (!cond) throw new Error(msg)
}
function near(a, b, msg, tol = 1e-9) {
  assert(Math.abs(a - b) <= tol, `${msg}: ${a} !== ${b}`)
}

console.log('AGL ground reference:')

// Vehicle hovering at 14.8 m AMSL, 4.0 m above where it armed (PX4 HOME at
// 10.8 m AMSL); the plan carries no HOME elevation.
const HOVER = { planHomeAmslM: 0, reportedAmslM: 14.8, relativeAltM: 4.0 }

check('plan HOME elevation wins when the plan has one', () => {
  const g = groundReferenceAmsl({ ...HOVER, planHomeAmslM: 12.3 })
  near(g, 12.3, 'ground')
  near(heightAboveGroundM({ ...HOVER, planHomeAmslM: 12.3 }), 2.5, 'AGL')
})

check('no plan HOME falls back to PX4 HOME (AMSL − relative_alt)', () => {
  near(groundReferenceAmsl(HOVER), 10.8, 'ground')
})

check('DEM/mesh ground under the vehicle wins over the HOME plane', () => {
  near(groundReferenceAmsl({ ...HOVER, demGroundAmslM: 6.5 }), 6.5, 'ground')
  near(heightAboveGroundM({ ...HOVER, demGroundAmslM: 6.5 }), 8.3, 'AGL')
})

check('flying over lower ground raises the AGL (terrain, not HOME)', () => {
  const overRidge = heightAboveGroundM({ ...HOVER, demGroundAmslM: 12.5 })
  const overValley = heightAboveGroundM({ ...HOVER, demGroundAmslM: 2.5 })
  near(overRidge, 2.3, 'ridge AGL')
  near(overValley, 12.3, 'valley AGL')
  assert(overValley > overRidge, 'lower ground must read a larger clearance')
})

check('outside DEM coverage (null) falls back to the HOME plane', () => {
  near(groundReferenceAmsl({ ...HOVER, demGroundAmslM: null }), 10.8, 'ground')
})

check('hovering reads the real clearance, not the AMSL altitude', () => {
  const agl = heightAboveGroundM(HOVER)
  near(agl, 4.0, 'AGL')
  assert(agl !== HOVER.reportedAmslM, 'AGL must not equal the AMSL altitude')
})

check('a landed vehicle reads 0 AGL, not its AMSL altitude', () => {
  const landed = { planHomeAmslM: 0, reportedAmslM: 3.3, relativeAltM: 0 }
  near(heightAboveGroundM(landed), 0, 'AGL')
})

check('an unknown AMSL falls back to the ellipsoid, not NaN', () => {
  const g = groundReferenceAmsl({ planHomeAmslM: 0, reportedAmslM: NaN, relativeAltM: 5 })
  near(g, 0, 'ground')
})

// ---------------------------------------------------------------------------
// Terrain drape over the 3D mesh: the survey AGL must land as AMSL above the
// mesh surface, in the mesh's own vertical datum.
// ---------------------------------------------------------------------------

/** Planned waypoint fixture (command 16 = NAV_WAYPOINT carries a coordinate). */
function wp(seq, meters, { command = 16, locked = false } = {}) {
  return {
    position: { latitude_deg: 48.5 + seq / 1000, longitude_deg: -123.4 },
    altitude: { datum: 'AMSL_EGM96', meters },
    command,
    params: [0, 0, 0, 0],
    autocontinue: true,
    locked,
  }
}

console.log('\nTerrain drape over the 3D mesh:')

check('a 40 m AGL drape lands 40 m above the mesh surface', () => {
  // Ellipsoid-datum mesh: 100 m of ellipsoid height, N(EGM96) = −19.5 m at
  // this longitude, so the ground is 119.5 m AMSL and the waypoint 159.5 m.
  const ground = new Map([[0, 100]])
  const geoid = new Map([[0, -19.5]])
  const r = drapeToMesh([wp(0, 40)], ground, { aglM: 40, geoidByIndex: geoid })
  near(r.waypoints[0].altitude.meters, 159.5, 'AMSL height')
  near(r.minAglM, 40, 'AGL flown')
  near(r.minGroundAmslM, 119.5, 'ground AMSL')
  near(r.waypoints[0].altitude.meters - 119.5, 40, 'clearance above ground')
})

check('an orthometric mesh needs no geoid conversion', () => {
  // CGVD2013-datum tileset: the sample already is ~AMSL.
  const r = drapeToMesh([wp(0, 0)], new Map([[0, 61.2]]), { aglM: 40 })
  near(r.waypoints[0].altitude.meters, 101.2, 'AMSL height')
  near(r.minAglM, 40, 'AGL flown')
})

check('every draped waypoint keeps the requested AGL across relief', () => {
  const ground = new Map([
    [0, 100],
    [1, 130],
    [2, 88],
  ])
  const geoid = new Map([
    [0, -19.5],
    [1, -19.5],
    [2, -19.5],
  ])
  const r = drapeToMesh([wp(0, 0), wp(1, 0), wp(2, 0)], ground, {
    aglM: 40,
    geoidByIndex: geoid,
  })
  r.waypoints.forEach((w, i) => {
    // AMSL = h − N, and N is negative here, so the ground sits *above* the
    // ellipsoid height the tileset serves.
    near(w.altitude.meters, ground.get(i) - geoid.get(i) + 40, `waypoint ${i} AMSL`)
  })
  assert(r.minAglM === 40 && r.draped === 3, 'all three draped at 40 AGL')
})

check('a locked waypoint keeps its hand-set height', () => {
  const r = drapeToMesh([wp(0, 77, { locked: true })], new Map([[0, 100]]), { aglM: 40 })
  near(r.waypoints[0].altitude.meters, 77, 'locked height')
  assert(r.draped === 0, 'nothing draped')
})

check('command items (no coordinate) pass through untouched', () => {
  const r = drapeToMesh([wp(0, 12, { command: 178 })], new Map([[0, 100]]), { aglM: 40 })
  near(r.waypoints[0].altitude.meters, 12, 'command param7')
  assert(r.draped === 0 && r.missing === 0, 'counted as neither')
})

check('a waypoint with no mesh sample stays as-is and is counted', () => {
  const r = drapeToMesh([wp(0, 0), wp(1, 50)], new Map([[0, 100]]), { aglM: 40 })
  near(r.waypoints[1].altitude.meters, 50, 'untouched height')
  assert(r.draped === 1 && r.missing === 1, 'one draped, one missing')
})

check('the return-climb floor lifts a valley waypoint', () => {
  const ground = new Map([
    [0, 100],
    [1, 20],
  ])
  const r = drapeToMesh([wp(0, 0), wp(1, 0)], ground, { aglM: 40, floorBelowStartM: 10 })
  near(r.waypoints[0].altitude.meters, 140, 'start height')
  near(r.waypoints[1].altitude.meters, 130, 'floored to start − 10')
})

check('a landing waypoint is draped onto the ground, not 40 m above it', () => {
  // Operator report (SITL): the landing target kept a HOME-plane height and
  // the aircraft hovered over lower ground instead of touching down.
  const ground = new Map([
    [0, 100],
    [1, 20],
  ])
  const geoid = new Map([
    [0, 0],
    [1, 0],
  ])
  // Cruise leg over the high ground, landing item at the low point.
  const plan = [wp(0, 0), wp(1, 0, { command: 21 })]
  const r = drapeToMesh(plan, ground, { aglM: 40, geoidByIndex: geoid })
  near(r.waypoints[0].altitude.meters, 140, 'the cruise leg keeps its AGL')
  near(r.waypoints[1].altitude.meters, 20, 'landing on the ground')
  assert(r.landed === 1, 'one landing item reported')
  assert(r.draped === 2, 'both rewritten')
})

check('a landing clearance may hold the target just above the ground', () => {
  const r = drapeToMesh([wp(0, 0, { command: 21 })], new Map([[0, 20]]), {
    aglM: 40,
    landClearanceM: 0.5,
  })
  near(r.waypoints[0].altitude.meters, 20.5, 'landing target')
  near(r.minAglM, 0.5, 'clearance flown')
})

check('a locked landing waypoint keeps its pin', () => {
  const r = drapeToMesh([wp(0, 77, { command: 21, locked: true })], new Map([[0, 100]]), {
    aglM: 40,
  })
  near(r.waypoints[0].altitude.meters, 77, 'locked height')
  assert(r.landed === 0 && r.draped === 0, 'nothing draped')
})

check('the drape never guesses: an empty sample set drapes nothing', () => {
  const r = drapeToMesh([wp(0, 33)], new Map(), { aglM: 40 })
  near(r.waypoints[0].altitude.meters, 33, 'untouched height')
  assert(r.draped === 0 && r.missing === 1, 'reported as uncovered')
})

check('the result leaves the input plan alone (no aliasing)', () => {
  const plan = [wp(0, 40)]
  const r = drapeToMesh(plan, new Map([[0, 100]]), { aglM: 40 })
  near(plan[0].altitude.meters, 40, 'input height')
  near(r.waypoints[0].altitude.meters, 140, 'draped height')
})

if (failures.length > 0) {
  console.error(`\ncheck:agl FAILED — ${failures.length} failure(s):`)
  for (const f of failures) console.error(`  ${f}`)
  process.exit(1)
}
console.log(`\ncheck:agl OK — ${passed} scenario(s)`)
