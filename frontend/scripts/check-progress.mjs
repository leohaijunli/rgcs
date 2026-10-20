#!/usr/bin/env node
// Flight-progress checks (issues.md #37).
//
//   node scripts/check-progress.mjs
//
// Pure modules, bundled with esbuild like the other checks: the great-circle
// helpers are validated against known geometry and the progress readout is
// driven through the states the operator sees while flying — plan known, fix
// known, fix lost, MISSION_CURRENT not yet received.
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')

const dir = await mkdtemp(join(tmpdir(), 'maggcs-progress-'))
async function bundle(entry, name) {
  const out = join(dir, name)
  await build({
    entryPoints: [join(srcDir, 'mission', entry)],
    outfile: out,
    bundle: true,
    format: 'esm',
    platform: 'neutral',
    logLevel: 'silent',
  })
  return import(`file://${out}`)
}
const { missionProgress, formatDuration, formatDistance } = await bundle('progress.ts', 'progress.mjs')
const { haversineM, bearingDeg, EARTH_RADIUS_M } = await bundle('geo.ts', 'geo.mjs')
await rm(dir, { recursive: true, force: true })

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
function near(a, b, msg, tol) {
  assert(
    Number.isFinite(a) && Math.abs(a - b) <= tol,
    `${msg}: ${a} !== ${b} (tol ${tol})`,
  )
}

const DEG = Math.PI / 180
/** One degree of latitude on the mean sphere. */
const DEG_LAT_M = EARTH_RADIUS_M * DEG
const HOME = { latitude_deg: 49.2606, longitude_deg: -123.246 }

/** A waypoint `nM` metres north of HOME, at `seq`. */
function north(seq, nM) {
  return {
    seq,
    latitude_deg: HOME.latitude_deg + nM / DEG_LAT_M,
    longitude_deg: HOME.longitude_deg,
    altitude_m: 80,
  }
}

console.log('geometry:')

check('the same point is zero metres away', () => {
  near(haversineM(HOME, HOME), 0, 'distance', 1e-6)
})

check('one degree of latitude is ~111.2 km', () => {
  const b = { latitude_deg: HOME.latitude_deg + 1, longitude_deg: HOME.longitude_deg }
  near(haversineM(HOME, b), DEG_LAT_M, 'distance', 1)
})

check('the east-west scale matches cos(latitude)', () => {
  const east = { latitude_deg: HOME.latitude_deg, longitude_deg: HOME.longitude_deg + 1 }
  const ratio = haversineM(HOME, east) / DEG_LAT_M
  near(ratio, Math.cos(HOME.latitude_deg * DEG), 'ratio', 1e-3)
})

check('distance is symmetric', () => {
  const far = { latitude_deg: 48.6493, longitude_deg: -123.3982 }
  near(haversineM(HOME, far), haversineM(far, HOME), 'distance', 1e-6)
})

check('cardinal bearings are 0/90/180/270', () => {
  // On the equator a due-east course is a great circle, so the initial
  // bearing is exactly 90°; at higher latitudes the sphere adds curvature.
  const eq = { latitude_deg: 0, longitude_deg: 0 }
  near(bearingDeg(eq, { latitude_deg: 0.01, longitude_deg: 0 }), 0, 'north', 1e-6)
  near(bearingDeg(eq, { latitude_deg: 0, longitude_deg: 0.01 }), 90, 'east', 1e-6)
  near(bearingDeg(eq, { latitude_deg: -0.01, longitude_deg: 0 }), 180, 'south', 1e-6)
  near(bearingDeg(eq, { latitude_deg: 0, longitude_deg: -0.01 }), 270, 'west', 1e-6)
})

console.log('\nprogress:')

const PLAN = [north(0, 0), north(1, 1000), north(2, 2000), north(3, 3000)]

check('no MISSION_CURRENT still reports the plan', () => {
  const p = missionProgress(PLAN, null, null, null)
  assert(p.total === 4, 'total')
  assert(p.targetIndex === null, 'target')
  near(p.remainingPathM, 3000, 'remaining path', 1)
  assert(p.fraction === 0, 'fraction')
  assert(p.etaS === null, 'eta')
})

check('a target on the second item counts the legs it leaves behind', () => {
  const p = missionProgress(PLAN, 1, null, 5)
  assert(p.targetIndex === 1, 'target')
  assert(p.flown === 1 && p.remaining === 3, `flown/remaining ${p.flown}/${p.remaining}`)
  near(p.remainingPathM, 2000, 'remaining path', 1)
  near(p.flownPathM, 1000, 'flown path', 1)
  near(p.fraction, 1 / 3, 'fraction', 1e-3)
  near(p.etaS, 400, 'eta', 1)
})

check('a live fix measures the last leg from where the vehicle is', () => {
  const vehicle = north(0, 2500)
  const p = missionProgress(PLAN, 3, vehicle, 5)
  near(p.toTargetM, 500, 'to target', 1)
  near(p.toTargetBearingDeg, 0, 'bearing', 1e-3)
  near(p.remainingPathM, 500, 'remaining path', 1)
  near(p.fraction, 3000 - 500 > 0 ? (3000 - 500) / 3000 : 0, 'fraction', 1e-3)
  near(p.targetAltitudeM, 80, 'target altitude', 1e-9)
  near(p.etaS, 100, 'eta', 1)
})

check('reaching the last item leaves nothing to fly', () => {
  const p = missionProgress(PLAN, 3, north(0, 3000), 5)
  near(p.remainingPathM, 0, 'remaining path', 1)
  near(p.fraction, 1, 'fraction', 1e-6)
  near(p.etaS, 0, 'eta', 1e-6)
})

check('a stationary vehicle has no ETA, not an infinite one', () => {
  const p = missionProgress(PLAN, 0, north(0, -10), 0.2)
  assert(p.etaS === null, `eta ${p.etaS}`)
})

check('a MISSION_CURRENT the plan does not contain is ignored', () => {
  const p = missionProgress(PLAN, 42, north(0, 0), 5)
  assert(p.targetIndex === null, 'target')
  near(p.remainingPathM, 3000, 'remaining path', 1)
  assert(p.toTargetM === null, 'to target')
})

check('non-contiguous seqs still resolve the target', () => {
  const sparse = [north(2, 0), north(5, 1000), north(7, 2000)]
  const p = missionProgress(sparse, 5, north(0, 500), 5)
  assert(p.targetIndex === 1, `target ${p.targetIndex}`)
  near(p.toTargetM, 500, 'to target', 1)
  near(p.remainingPathM, 1500, 'remaining path', 1)
})

check('a command item without a position does not break the leg maths', () => {
  // progress.ts only receives coordinate items; a plan whose first item is a
  // command item is compiled to positions only, so seqs start at 0 here.
  const p = missionProgress([north(0, 0)], 0, north(0, 0), 5)
  near(p.fraction, 0, 'fraction', 1e-6)
  assert(p.total === 1, 'total')
})

check('an empty plan is all zeros', () => {
  const p = missionProgress([], 0, HOME, 5)
  assert(p.total === 0, 'total')
  assert(p.targetIndex === null, 'target')
  assert(p.remainingPathM === 0 && p.fraction === 0, 'path')
})

check('formatDistance switches unit at a kilometre', () => {
  assert(formatDistance(412.4) === '412 m', formatDistance(412.4))
  assert(formatDistance(1840) === '1.84 km', formatDistance(1840))
  assert(formatDistance(0) === '0 m', formatDistance(0))
  assert(formatDistance(null) === null, 'null')
})

check('formatDuration renders m:ss and rejects nonsense', () => {
  assert(formatDuration(100) === '1:40', formatDuration(100))
  assert(formatDuration(9) === '0:09', formatDuration(9))
  assert(formatDuration(null) === null, 'null')
  assert(formatDuration(-1) === null, 'negative')
})

if (failures.length > 0) {
  console.error(`\ncheck:progress FAILED — ${failures.length} failure(s):`)
  for (const f of failures) console.error(`  ${f}`)
  process.exit(1)
}
console.log(`\ncheck:progress OK — ${passed} scenario(s)`)
