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

const { groundReferenceAmsl, heightAboveGroundM, AGL_GROUND_BAND_M, isBelowGround, vehicleAmslAnchoredM } =
  await load('cesium/heights.ts')
// A9: the compiled solver route is applied to the plan by the pure merge in
// `mission/drapeApply.ts` (the interim per-waypoint drape was deleted).
const { applyCompiled } = await load('mission/drapeApply.ts')

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

// Operator report: on the ground at takeoff the track was coloured "below
// ground" because the reference is never exact (DEM/DTM error vs PX4 AMSL, or
// estimator noise in `relative_alt` while disarmed). Only a real negative
// beyond the ground band counts as below the surface.
console.log('\nOn-the-ground band (track colour / drop line agreement):')

check('a landed vehicle with a small negative AGL is NOT below ground', () => {
  assert(AGL_GROUND_BAND_M > 0, 'band must be positive')
  assert(!isBelowGround(-0.4), 'decimetre noise while landed must read as on the ground')
  assert(!isBelowGround(-AGL_GROUND_BAND_M + 1e-6), 'just inside the band is on the ground')
})

check('zero and positive AGL are never below ground', () => {
  assert(!isBelowGround(0), 'zero AGL is the surface')
  assert(!isBelowGround(12.5), 'airborne is above ground')
})

check('only a real negative beyond the band counts as below ground', () => {
  assert(isBelowGround(-AGL_GROUND_BAND_M - 1e-6), 'just past the band is below ground')
  assert(isBelowGround(-8), 'a valley incursion is below ground')
})

check('non-finite AGL is never reported as below ground', () => {
  assert(!isBelowGround(NaN), 'unknown height must not paint as an error')
  assert(!isBelowGround(Infinity), 'infinite height is above ground')
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

/** The position key `applyCompiled` matches landing items by (same helper). */
function landKey(seq) {
  const w = wp(seq, 0)
  return `${w.position.latitude_deg.toFixed(9)},${w.position.longitude_deg.toFixed(9)}`
}

console.log('\nA9 drape apply (compiled route → plan):')

/** Compiled route item as the backend returns it (PlannedWaypoint, AMSL). */
function compiledItem(seq, meters, { command = 16, lineId = 'survey-1', locked = false } = {}) {
  return {
    waypoint: {
      position: { latitude_deg: 48.5 + seq / 1000, longitude_deg: -123.4 },
      altitude: { datum: 'AMSL_EGM96', meters },
      command,
      params: [0, 0, 0, 0],
      autocontinue: true,
      locked,
    },
    s: seq * 10,
    line_id: lineId,
    kind: lineId.split('-')[0],
  }
}

/** A generated pattern block over seqs 0..2 with the backend route applied. */
function run(current, compiled, landing = new Map(), block = { start: 0, end: 2 }) {
  return applyCompiled(current, block, compiled, landing)
}

check('the compiled route replaces the pattern block in order', () => {
  const plan = [wp(0, 0), wp(1, 0), wp(2, 0)]
  const compiled = [compiledItem(0, 140), compiledItem(1, 143), compiledItem(2, 128)]
  const r = run(plan, compiled)
  near(r.waypoints[0].altitude.meters, 140, 'route item 0 AMSL')
  near(r.waypoints[1].altitude.meters, 143, 'route item 1 AMSL')
  near(r.waypoints[2].altitude.meters, 128, 'route item 2 AMSL')
  assert(r.applied === 3, 'all three applied')
})

check('items outside the pattern block pass through untouched', () => {
  const home = { ...wp(0, 5), command: 16 }
  const plan = [home, wp(1, 0), wp(2, 0), wp(3, 0)]
  const compiled = [compiledItem(1, 143), compiledItem(2, 128)]
  const r = run(plan, compiled, new Map(), { start: 1, end: 3 })
  near(r.waypoints[0].altitude.meters, 5, 'pre-block item untouched')
  near(r.waypoints[1].altitude.meters, 143, 'route replaces the block')
  near(r.waypoints[2].altitude.meters, 128, 'route replaces the block')
  assert(r.waypoints.length === 3, 'block length follows the compiled route')
})

check('a pinned height inside the block survives (restored onto the route)', () => {
  const pinned = wp(1, 77, { locked: true })
  const plan = [wp(0, 0), pinned, wp(2, 0)]
  const compiled = [compiledItem(0, 140), compiledItem(1, 143), compiledItem(2, 128)]
  const r = run(plan, compiled)
  near(r.waypoints[1].altitude.meters, 77, 'the pin survives')
  assert(r.waypoints[1].locked, 'still pinned')
  assert(r.pinned === 1, 'one pin restored')
  assert(!r.waypoints[0].locked && !r.waypoints[2].locked, 'the rest are free')
})

check('a pinned landing item keeps its pin (never re-grounded)', () => {
  const pinnedLand = { ...wp(0, 77, { locked: true }), command: 21 }
  const plan = [wp(0, 0), pinnedLand]
  const compiled = [compiledItem(0, 140)]
  const r = run(plan, compiled, new Map([[landKey(0), 20]]), { start: 0, end: 0 })
  near(r.waypoints[1].altitude.meters, 77, 'pinned landing untouched')
  assert(r.landed === 0, 'not counted as re-grounded')
})

check('landing targets are placed on the DEM surface (no AGL, no offset)', () => {
  // Operator report (SITL): a landing whose target sat on the HOME plane left
  // the vehicle hovering over lower ground. The backend returns the surface
  // height (AMSL); the merge rewrites only the landing items.
  const plan = [wp(0, 0), { ...wp(0, 41), command: 21 }]
  const compiled = [compiledItem(0, 142)] // survey leg: ground + agl + offset
  const landing = new Map([[landKey(0), 20]])
  const r = run(plan, compiled, landing, { start: 0, end: 0 })
  near(r.waypoints[0].altitude.meters, 142, 'the survey leg keeps its AMSL')
  near(r.waypoints[1].altitude.meters, 20, 'landing on the DEM surface')
  assert(r.landed === 1, 'one landing item re-grounded')
})

check('a landing item with no DEM sample is left as-is (fail-open per item)', () => {
  const plan = [wp(0, 0), { ...wp(0, 41), command: 21 }]
  const r = run(plan, [compiledItem(0, 142)], new Map(), { start: 0, end: 0 })
  near(r.waypoints[1].altitude.meters, 41, 'untouched height')
  assert(r.landed === 0, 'nothing re-grounded')
})

check('the vertical offset lives in the compiled heights, not the merge', () => {
  // DR1: the operator offset rides on the AGL target — it is baked into the
  // backend heights; the merge must not add or remove it. The landing target
  // still aims at the ground itself.
  const plan = [wp(0, 40), { ...wp(0, 41), command: 21 }]
  const compiled = [compiledItem(0, 142)] // ground 100 + 40 AGL + 2 offset
  const r = run(plan, compiled, new Map([[landKey(0), 100]]), { start: 0, end: 0 })
  near(r.waypoints[0].altitude.meters, 142, 'survey: ground + agl + offset')
  near(r.waypoints[1].altitude.meters, 100, 'landing: the ground itself')
})

check('the line table re-anchors to the new block (map line colours)', () => {
  const plan = [wp(0, 0), wp(1, 0), wp(2, 0)]
  const compiled = [
    compiledItem(0, 140, { lineId: 'survey-1' }),
    compiledItem(1, 143, { lineId: 'survey-1' }),
    compiledItem(2, 128, { lineId: 'tie-1' }),
  ]
  const r = run(plan, compiled)
  assert(r.lines.length === 2, 'two line rows')
  const survey = r.lines.find((l) => l.kind === 'survey')
  const tie = r.lines.find((l) => l.kind === 'tie')
  assert(survey.start_seq === 0 && survey.end_seq === 1, 'survey range')
  assert(tie.start_seq === 2 && tie.end_seq === 2, 'tie range')
  assert(r.blockStart === 0 && r.blockEnd === 2, 'block bounds')
})

check('the result leaves the input plan alone (no aliasing)', () => {
  const plan = [wp(0, 40)]
  const r = run(plan, [compiledItem(0, 140)], new Map(), { start: 0, end: 0 })
  near(plan[0].altitude.meters, 40, 'input height')
  near(r.waypoints[0].altitude.meters, 140, 'applied height')
  assert(!plan[0].locked, 'input pin state untouched')
})

// --- vehicle AMSL anchoring (operator report 2026-10-18: PX4's EKF origin
// ~15 m low on a BC site — takeoff AGL −14 m, waypoints ~20 m above the drone)

const SIDNEY = {
  demGroundAtHomeM: 17.7, // DEM ground under HOME
  reportedAmslM: 3.0, // PX4 `alt` with its unanchored EKF origin
  relativeAltM: 40.0, // exact against the same origin
}

check('the vehicle lands in the DEM frame, not the EKF-origin frame', () => {
  near(vehicleAmslAnchoredM(SIDNEY), 57.7, 'home ground + relative_alt')
})

check('without a HOME ground the reported AMSL is the fallback', () => {
  near(
    vehicleAmslAnchoredM({ ...SIDNEY, demGroundAtHomeM: null }),
    3.0,
    'reported AMSL passthrough',
  )
  near(vehicleAmslAnchoredM({ ...SIDNEY, demGroundAtHomeM: undefined }), 3.0, 'undefined passthrough')
  near(
    vehicleAmslAnchoredM({ ...SIDNEY, relativeAltM: Number.NaN }),
    3.0,
    'non-finite relative_alt falls back',
  )
})

check('a properly anchored origin is untouched (correction ≈ 0)', () => {
  near(
    vehicleAmslAnchoredM({ demGroundAtHomeM: 17.7, reportedAmslM: 57.7, relativeAltM: 40.0 }),
    57.7,
    'anchored origin agrees with the DEM',
  )
})

check('the landed self-calibration needs no HOME_POSITION fix', () => {
  // The field case: PX4 never streamed HOME_POSITION, so the HOME-ground
  // anchor never engaged. The landed calibration does not need it.
  const anchored = vehicleAmslAnchoredM({
    anchorOffsetM: 14.7,
    demGroundAtHomeM: null,
    reportedAmslM: 3.0,
    relativeAltM: 0.0,
  })
  near(anchored, 17.7, 'offset + reported')
})

check('landing at B (different elevation) still reads AGL 0', () => {
  // Takeoff at A: ground 17.7, reported 3 → offset 14.7. Fly to B where the
  // DEM ground is 42.1 and land: the vehicle reports 42.1 − 14.7 = 27.4 in
  // its frame; the DEM under B cancels the bias.
  const offset = 17.7 - 3.0
  const reportedB = 42.1 - 14.7
  const anchoredB = vehicleAmslAnchoredM({
    anchorOffsetM: offset,
    demGroundAtHomeM: null,
    reportedAmslM: reportedB,
    relativeAltM: 0.0,
  })
  near(anchoredB, 42.1, 'AMSL at B in the DEM frame')
  const aglB = heightAboveGroundM({
    demGroundAmslM: 42.1,
    planHomeAmslM: 0,
    reportedAmslM: anchoredB,
    relativeAltM: 0.0,
  })
  near(aglB, 0, 'AGL at B')
})

check('the AGL readout anchors to zero at takeoff and 40 in flight', () => {
  // Takeoff: relative 0 over the HOME ground → AGL 0 (was reported − ground = −14.7).
  const takeoff = heightAboveGroundM({
    demGroundAmslM: 17.7,
    planHomeAmslM: 0,
    reportedAmslM: vehicleAmslAnchoredM({ ...SIDNEY, relativeAltM: 0.0 }),
    relativeAltM: 0.0,
  })
  near(takeoff, 0, 'takeoff AGL')
  // In flight over the same ground: relative 40 → AGL 40 (the draped setting).
  const flight = heightAboveGroundM({
    demGroundAmslM: 18.2,
    planHomeAmslM: 0,
    reportedAmslM: vehicleAmslAnchoredM(SIDNEY),
    relativeAltM: 40.0,
  })
  near(flight, 39.5, 'flight AGL over slightly higher ground')
})

if (failures.length > 0) {
  console.error(`\ncheck:agl FAILED — ${failures.length} failure(s):`)
  for (const f of failures) console.error(`  ${f}`)
  process.exit(1)
}
console.log(`\ncheck:agl OK — ${passed} scenario(s)`)
