#!/usr/bin/env node
// Survey AOI + plan altitude profile (the pure cores of the DEM window and
// the plan-review chart).
//
//   node scripts/check-plan-profile.mjs
//
// `effectiveSurveyAoi` (util/surveyAoi.ts) must cover the **drawn boundary** —
// a closed polygon's bounding box wins over the rectangle inputs, or a 6 ha
// polygon gets a 100 × 100 m DEM and most waypoints fall outside the mesh.
// `profileSeries` (mission/profile.ts) must sample the profile along the flown
// path (not only at waypoints) and key the ground samples by the waypoint's
// index in the full list — the `seq` the compiled mission items use.
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')

async function bundle(entry) {
  const out = join(await mkdtemp(join(tmpdir(), 'maggcs-plan-profile-')), 'mod.mjs')
  await build({
    entryPoints: [join(srcDir, entry)],
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

const { profileSeries } = await bundle('mission/profile.ts')
const { effectiveSurveyAoi, polygonAoi } = await bundle('util/surveyAoi.ts')

const failures = []
let passed = 0
function check(name, fn) {
  try {
    fn()
    passed += 1
    console.log(`  ok   ${name}`)
  } catch (e) {
    failures.push(name)
    console.log(`  FAIL ${name}: ${e.message}`)
  }
}
function eq(got, want, what) {
  if (got !== want) throw new Error(`${what}: want ${JSON.stringify(want)}, got ${JSON.stringify(got)}`)
}
function approx(got, want, tol, what) {
  if (!(Math.abs(got - want) <= tol)) {
    throw new Error(`${what}: want ${want} ±${tol}, got ${got}`)
  }
}

const wp = (lat, lon, alt, command = 16) => ({
  position: { latitude_deg: lat, longitude_deg: lon },
  altitude: { meters: alt },
  command,
  params: [0, 0, 0, 0],
  autocontinue: true,
})

// --- profileSeries ---------------------------------------------------------

check('needs two flyable waypoints', () => {
  eq(profileSeries([wp(48.5, -123.4, 50)], null), null)
  eq(profileSeries([], null), null)
})

check('samples along the path, not only at waypoints', () => {
  // ~1.1 km leg: the waypoint-only polyline would be 2 points.
  const s = profileSeries([wp(48.5, -123.4, 50), wp(48.51, -123.4, 50)], null)
  if (!s) throw new Error('no series')
  if (s.dist.length < 100) throw new Error(`only ${s.dist.length} samples`)
  eq(s.dist[0], 0, 'starts at 0')
  // 0.01° of latitude on the haversine sphere (R = 6371 km) ≈ 1111.95 m.
  approx(s.dist.at(-1), 1111.95, 1.5, 'ends at the path length')
})

check('interpolates the planned height linearly', () => {
  const s = profileSeries([wp(48.5, -123.4, 40), wp(48.5, -123.3, 60)], null)
  if (!s) throw new Error('no series')
  const mid = s.amsl[Math.floor(s.amsl.length / 2)]
  approx(mid, 50, 0.2, 'midpoint height')
})

check('keys the ground by the full-list index (seq)', () => {
  // A non-waypoint command sits at index 1: the ground for the second
  // waypoint must come from index 2, not shift down one.
  const items = [wp(48.5, -123.4, 50), wp(48.5, -123.39, 50, 99), wp(48.5, -123.38, 50)]
  const s = profileSeries(items, { 0: 100, 2: 200 })
  if (!s) throw new Error('no series')
  approx(s.ground[0], 100, 1e-9, 'ground at the start')
  approx(s.ground.at(-1), 200, 1e-9, 'ground at the end')
  if (s.ground.every((g) => g == null)) throw new Error('ground never resolved')
})

check('ground stays null across an unsampled waypoint', () => {
  const s = profileSeries([wp(48.5, -123.4, 50), wp(48.5, -123.38, 50)], { 0: 100 })
  if (!s) throw new Error('no series')
  if (!s.ground.at(-1) == null === false && s.ground.at(-1) != null) {
    throw new Error('last sample should have no ground')
  }
  const middles = s.ground.slice(10, -10)
  if (middles.every((g) => g != null)) throw new Error('gap should be null')
})

check('caps the sample count on long plans', () => {
  // ~111 km path: samples must stay bounded.
  const s = profileSeries([wp(48.0, -123.4, 50), wp(49.0, -123.4, 50)], null)
  if (!s) throw new Error('no series')
  if (s.dist.length > 2001) throw new Error(`${s.dist.length} samples`)
})

// --- effectiveSurveyAoi / polygonAoi ---------------------------------------

const box = {
  vertices: [
    { latitude_deg: 48.5, longitude_deg: -123.38 },
    { latitude_deg: 48.51, longitude_deg: -123.38 },
    { latitude_deg: 48.51, longitude_deg: -123.37 },
    { latitude_deg: 48.5, longitude_deg: -123.37 },
  ],
  closed: true,
}

check('a closed polygon wins with its bounding box', () => {
  const aoi = effectiveSurveyAoi(box, { latitude_deg: 48.505, longitude_deg: -123.375 }, { width: 100, height: 100 })
  if (!aoi) throw new Error('no aoi')
  approx(aoi.lat, 48.505, 1e-9, 'centre lat')
  approx(aoi.lon, -123.375, 1e-9, 'centre lon')
  // 0.01° of longitude at 48.505 N ≈ 737.6 m; 0.01° of latitude ≈ 1113.2 m.
  approx(aoi.width, 737.6, 1.5, 'bbox width')
  approx(aoi.height, 1113.2, 1.5, 'bbox height')
})

check('an open polygon falls back to the rectangle', () => {
  const aoi = effectiveSurveyAoi({ ...box, closed: false }, { latitude_deg: 48.5, longitude_deg: -123.4 }, { width: 300, height: 200 })
  if (!aoi) throw new Error('no aoi')
  approx(aoi.lat, 48.5, 1e-9, 'lat')
  approx(aoi.lon, -123.4, 1e-9, 'lon')
  eq(aoi.width, 300, 'width')
  eq(aoi.height, 200, 'height')
})

check('no polygon and no centre is no aoi', () => {
  eq(effectiveSurveyAoi(null, null, { width: 100, height: 100 }), null)
})

check('a degenerate polygon falls back to the rectangle', () => {
  const aoi = effectiveSurveyAoi(
    { vertices: [{ latitude_deg: 48.5, longitude_deg: -123.4 }], closed: true },
    { latitude_deg: 48.6, longitude_deg: -123.4 },
    { width: 100, height: 100 },
  )
  approx(aoi.lat, 48.6, 1e-9, 'lat')
  eq(aoi.width, 100, 'width')
})

check('polygonAoi ignores vertex order', () => {
  const a = polygonAoi(box.vertices)
  const b = polygonAoi([...box.vertices].reverse())
  approx(a.width, b.width, 1e-9, 'width')
  approx(a.height, b.height, 1e-9, 'height')
})

if (failures.length > 0) {
  console.log(`\n${failures.length} failure(s)`)
  process.exit(1)
}
console.log(`\n${passed} passed`)
