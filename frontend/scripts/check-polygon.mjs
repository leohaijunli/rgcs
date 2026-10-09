#!/usr/bin/env node
// Survey-polygon geometry checks (WS-G G2 / B3): the map tool uses these to
// flag a bad boundary before Generate reaches Rust. Golden numbers are exact in
// the local tangent plane, mirroring core::survey::validate_polygon.
//
//   node scripts/check-polygon.mjs
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')

const out = join(await mkdtemp(join(tmpdir(), 'maggcs-polygon-')), 'mod.mjs')
await build({
  entryPoints: [join(srcDir, 'mission', 'polygon.ts')],
  outfile: out,
  bundle: true,
  format: 'esm',
  platform: 'neutral',
  logLevel: 'silent',
})
const { polygonArea, polygonPerimeter, selfIntersects } = await import(`file://${out}`)
await rm(dirname(out), { recursive: true, force: true })

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
function near(a, b, msg, tol = 1) {
  assert(Math.abs(a - b) <= tol, `${msg}: ${a} !== ${b} (±${tol})`)
}
const p = (lat, lon) => ({ latitude_deg: lat, longitude_deg: lon })

// Golden shapes written in degrees around 48 N; distances are exact in the
// local tangent plane regardless of the origin.
const LAT = 48.0
const LON = -123.0
const mN = (m) => m / 111319.5
const mE = (m) => m / (111319.5 * Math.cos((LAT * Math.PI) / 180))

console.log('survey polygon geometry:')

check('a 500 x 500 m square has the expected area and perimeter', () => {
  const sq = [
    p(LAT, LON),
    p(LAT, LON + mE(500)),
    p(LAT + mN(500), LON + mE(500)),
    p(LAT + mN(500), LON),
  ]
  // The equirectangular projection is not exactly equal-area: allow 0.05 %.
  near(polygonArea(sq), 250000, 'area m²', 125)
  near(polygonPerimeter(sq), 2000, 'perimeter m', 2)
})

check('a concave U-shape subtracts the notch', () => {
  const u = [
    p(LAT, LON),
    p(LAT, LON + mE(600)),
    p(LAT + mN(600), LON + mE(600)),
    p(LAT + mN(600), LON + mE(400)),
    p(LAT + mN(200), LON + mE(400)),
    p(LAT + mN(200), LON + mE(200)),
    p(LAT + mN(600), LON + mE(200)),
    p(LAT + mN(600), LON),
  ]
  // 600x600 outer minus the 200x400 notch.
  near(polygonArea(u), 360000 - 80000, 'area m²', 150)
  // Outer square (2400) plus the two notch walls (400 + 400).
  near(polygonPerimeter(u), 2400 + 800, 'perimeter m', 2)
})

check('area is order-independent (same shape, reversed winding)', () => {
  const sq = [
    p(LAT, LON),
    p(LAT, LON + mE(500)),
    p(LAT + mN(500), LON + mE(500)),
    p(LAT + mN(500), LON),
  ]
  near(polygonArea(sq), polygonArea([...sq].reverse()), 'area', 1)
})

check('a bowtie is flagged self-intersecting', () => {
  const bowtie = [
    p(LAT, LON),
    p(LAT + mN(500), LON + mE(500)),
    p(LAT, LON + mE(500)),
    p(LAT + mN(500), LON),
  ]
  assert(selfIntersects(bowtie), 'expected a crossing')
})

check('a simple polygon is not self-intersecting', () => {
  const u = [
    p(LAT, LON),
    p(LAT, LON + mE(600)),
    p(LAT + mN(600), LON + mE(600)),
    p(LAT + mN(600), LON + mE(400)),
    p(LAT + mN(200), LON + mE(400)),
    p(LAT + mN(200), LON + mE(200)),
    p(LAT + mN(600), LON + mE(200)),
    p(LAT + mN(600), LON),
  ]
  assert(!selfIntersects(u), 'the U-shape is simple')
})

check('three vertices can never self-intersect', () => {
  assert(!selfIntersects([p(0, 0), p(1, 0), p(0, 1)]), 'triangle')
  assert(!selfIntersects([]), 'empty')
  assert(!selfIntersects([p(0, 0)]), 'single')
})

if (failures.length > 0) {
  console.error(`\ncheck:polygon FAILED — ${failures.length} failure(s):`)
  for (const f of failures) console.error(`  ${f}`)
  process.exit(1)
}
console.log(`\ncheck:polygon OK — ${passed} scenario(s)`)