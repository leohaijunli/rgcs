#!/usr/bin/env node
// N9 regression: Inspector magnetometer axes and derived rings are partitioned
// by the full MAVLink source identity (system, component, message).

import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'

const source = await readFile(new URL('../src/inspector/mag.ts', import.meta.url), 'utf8')
const compiled = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
}).outputText
const {
  calibratedMagRings,
  magComponentGroups,
  magTotalTraceId,
} = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`)

const signal = (message_id, field, system_id = 1, component_id = 1) => ({
  signal: { system_id, component_id, message_id, field },
})

// Deliberately interleave all axes. Grouping by field name alone would create
// one mixed x/y/z triple; source-aware grouping must produce three triples.
const traces = [
  signal(26, 'xmag'),
  signal(129, 'xmag'),
  signal(116, 'xmag'),
  signal(116, 'ymag'),
  signal(26, 'ymag'),
  signal(129, 'ymag'),
  signal(129, 'zmag'),
  signal(26, 'zmag'),
  signal(116, 'zmag'),
  signal(129, 'xmag', 2, 1), // incomplete fourth source: must not be combined
]
const groups = magComponentGroups(traces)
assert.deepEqual(
  groups.map(({ source, x, y, z }) => ({ source, x, y, z })),
  [
    { source: { system_id: 1, component_id: 1, message_id: 26 }, x: 0, y: 4, z: 7 },
    { source: { system_id: 1, component_id: 1, message_id: 129 }, x: 1, y: 5, z: 6 },
    { source: { system_id: 1, component_id: 1, message_id: 116 }, x: 2, y: 3, z: 8 },
  ],
)

const imu1Id = magTotalTraceId(groups[0].source)
const imu3Id = magTotalTraceId(groups[1].source)
const imu2Id = magTotalTraceId(groups[2].source)
assert.equal(imu1Id, 'mag_total_cal:1:1:26')
assert.equal(imu2Id, 'mag_total_cal:1:1:116')
assert.equal(imu3Id, 'mag_total_cal:1:1:129')
assert.equal(new Set([imu1Id, imu2Id, imu3Id]).size, 3)

const rings = new Map([
  [imu1Id, [10_000]],
  [imu2Id, [20_000]],
  [imu3Id, [30_000]],
])
const before1 = [...rings.get(imu1Id)]
const before2 = [...rings.get(imu2Id)]
rings.get(imu3Id).push(80_000) // offset/change only IMU3

const selected = calibratedMagRings(groups, rings)
assert.equal(selected[0], rings.get(imu1Id))
assert.equal(selected[1], rings.get(imu3Id))
assert.equal(selected[2], rings.get(imu2Id))
assert.deepEqual(rings.get(imu1Id), before1)
assert.deepEqual(rings.get(imu2Id), before2)
assert.deepEqual(rings.get(imu3Id), [30_000, 80_000])

console.log('check-inspector-mag OK')
