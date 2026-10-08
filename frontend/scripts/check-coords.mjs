#!/usr/bin/env node
// Coordinate-entry checks (improve_plan WS-G G2).
//
//   node scripts/check-coords.mjs
//
// Pure module, bundled with esbuild like the other checks: asserts the forms an
// operator can paste from a handheld GPS or a report, and that anything
// ambiguous or out of range is refused rather than silently misplaced.
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')

const out = join(await mkdtemp(join(tmpdir(), 'maggcs-coords-')), 'mod.mjs')
await build({
  entryPoints: [join(srcDir, 'mission', 'coords.ts')],
  outfile: out,
  bundle: true,
  format: 'esm',
  platform: 'neutral',
  logLevel: 'silent',
})
const { parseCoordinates, formatDms, formatDecimal } = await import(`file://${out}`)
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
function near(a, b, msg, tol = 1e-7) {
  assert(Math.abs(a - b) <= tol, `${msg}: ${a} !== ${b}`)
}
function ok(input, msg) {
  const r = parseCoordinates(input)
  assert(r.ok, `${msg ?? input}: expected a parse, got ${r.ok ? '' : r.error}`)
  return r.value
}
function fails(input, error, msg) {
  const r = parseCoordinates(input)
  assert(!r.ok, `${msg ?? input}: expected a failure, got ${JSON.stringify(r)}`)
  assert(r.error === error, `${msg ?? input}: expected ${error}, got ${r.error}`)
}

const HOME = { lat: 49.2606, lon: -123.246 }

console.log('coordinate entry:')

check('decimal degrees, comma separated', () => {
  const v = ok('48.6493, -123.3982')
  near(v.lat, 48.6493, 'lat')
  near(v.lon, -123.3982, 'lon')
  assert(v.format === 'decimal', 'format')
})

check('hemisphere letters set the axis and the sign', () => {
  const v = ok('49.2606N, 123.2460W')
  near(v.lat, 49.2606, 'lat')
  near(v.lon, -123.246, 'lon')
})

check('hemisphere letters survive a reversed pair', () => {
  const v = ok('123.2460W 49.2606N'.replace(' ', ', '))
  near(v.lat, 49.2606, 'lat')
  near(v.lon, -123.246, 'lon')
})

check('degrees, minutes and seconds', () => {
  const v = ok(`48°38'57.5"N, 123°23'53.5"W`)
  near(v.lat, 48 + 38 / 60 + 57.5 / 3600, 'lat', 1e-9)
  near(v.lon, -(123 + 23 / 60 + 53.5 / 3600), 'lon', 1e-9)
  assert(v.format === 'dms', 'format')
})

check('D/M/S with spaces or semicolons', () => {
  const v = ok('48 38 57.5 N; 123 23 53.5 W')
  near(v.lat, 48 + 38 / 60 + 57.5 / 3600, 'lat', 1e-9)
  near(v.lon, -(123 + 23 / 60 + 53.5 / 3600), 'lon', 1e-9)
})

check('a signed D/M/S triple keeps its own sign', () => {
  const v = ok(`-48 38 57.5, -123 23 53.5`)
  near(v.lat, -(48 + 38 / 60 + 57.5 / 3600), 'lat', 1e-9)
  near(v.lon, -(123 + 23 / 60 + 53.5 / 3600), 'lon', 1e-9)
})

check('a bare pair in longitude-latitude order is corrected', () => {
  // |−123.3982| > 90, so it cannot be a latitude.
  const v = ok('-123.3982, 48.6493')
  near(v.lat, 48.6493, 'lat')
  near(v.lon, -123.3982, 'lon')
})

check('whitespace and trailing separators are tolerated', () => {
  const v = ok('  48.6493 ,  -123.3982  ')
  near(v.lat, 48.6493, 'lat')
  near(v.lon, -123.3982, 'lon')
})

check('an empty field is its own error, not a format error', () => {
  fails('   ', 'empty')
})

check('a single number is refused', () => {
  fails('48.6493', 'format')
})

check('out-of-range values are refused', () => {
  fails('91.0, -123.0', 'range')
  fails('48.6, -181.0', 'range')
})

check('two latitudes or two longitudes are refused', () => {
  fails('48.6N, 49.2N', 'format')
  fails('123.4W, 124.5E', 'format')
})

check('typos are refused', () => {
  fails('48.6493, -123.3982abc', 'format')
  fails('48abc, -123.4', 'format')
  fails(`48°38'75"N, 123°23'53"W`, 'format')
})

check('formatDms round-trips through the parser', () => {
  const text = `${formatDms(HOME.lat, 'lat')}, ${formatDms(HOME.lon, 'lon')}`
  const v = ok(text)
  // 0.1" of rounding is 2.8e-5 deg.
  near(v.lat, HOME.lat, 'lat', 1e-4)
  near(v.lon, HOME.lon, 'lon', 1e-4)
})

check('formatDms carries a 60.0" reading into the minutes', () => {
  const v = ok(`${formatDms(48 + 59 / 60 + 59.97 / 3600, 'lat')}, 0`)
  assert(!v.lon && v.lon === 0, 'lon zero')
  assert(formatDms(48 + 59 / 60 + 59.97 / 3600, 'lat').includes("0'0.0\""), 'no 60 seconds')
})

check('formatDecimal is fixed precision', () => {
  assert(formatDecimal(-123.3982) === '-123.398200', formatDecimal(-123.3982))
})

check('a southern/western pair lands in the right hemisphere', () => {
  const v = ok(`33°52'4.0"S, 151°12'26.0"E`)
  assert(v.lat < 0 && v.lon > 0, `${v.lat}, ${v.lon}`)
})

if (failures.length > 0) {
  console.error(`\ncheck:coords FAILED — ${failures.length} failure(s):`)
  for (const f of failures) console.error(`  ${f}`)
  process.exit(1)
}
console.log(`\ncheck:coords OK — ${passed} scenario(s)`)
