#!/usr/bin/env node
// Plan edit undo/redo checks (U8).
//
//   node scripts/check-undo.mjs
//
// Bundles the real mission store (stubbing the Tauri plugins, which have no
// runtime in Node) and drives plan edits through the undo/redo stacks. The
// regression guard is the round-trip: edit → undo restores the *prior* plan →
// redo restores the edit; a fresh edit after an undo drops the redo branch.
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')

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
        'export const open=async()=>null;export const save=async()=>null;export const invoke=async()=>{};' +
        'export const listen=async()=>()=>{};',
      loader: 'js',
    }))
  },
}

async function bundle(entry) {
  const out = join(await mkdtemp(join(tmpdir(), 'maggcs-undo-')), 'mod.mjs')
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

const { useMissionStore } = await bundle('stores/mission.ts')

const failures = []
let passed = 0
async function check(name, fn) {
  try {
    await fn()
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
function eq(a, b, msg) {
  assert(a === b, `${msg ?? 'not equal'}: ${JSON.stringify(a)} !== ${JSON.stringify(b)}`)
}

const store = () => useMissionStore.getState()
const reset = () => store().reset()

await check('an add then undo removes the waypoint, redo restores it', () => {
  reset()
  store().addWaypointAt(48.6493, -123.3982)
  store().addWaypointAt(48.65, -123.397)
  eq(store().waypoints.length, 2, 'two waypoints')
  eq(store().undoStack.length, 2, 'two history entries')

  store().undo()
  eq(store().waypoints.length, 1, 'undo removes the last add')
  eq(store().undoStack.length, 1, 'undo stack shrinks')
  eq(store().redoStack.length, 1, 'redo stack grows')

  store().redo()
  eq(store().waypoints.length, 2, 'redo restores the add')
  eq(store().redoStack.length, 0, 'redo stack drains')
})

await check('undo past the beginning and redo past the end are no-ops', () => {
  reset()
  store().addWaypointAt(48.6493, -123.3982)
  store().undo()
  store().undo()
  eq(store().waypoints.length, 0, 'empty plan stays empty')
  eq(store().undoStack.length, 0, 'undo stack empty')
  store().redo()
  eq(store().waypoints.length, 1, 'redo restores')
  store().redo()
  eq(store().waypoints.length, 1, 'extra redo is a no-op')
})

await check('a new edit after an undo drops the redo branch', () => {
  reset()
  store().addWaypointAt(48.6493, -123.3982)
  store().undo()
  eq(store().redoStack.length, 1, 'redo available')
  // A fresh edit invalidates the undone future.
  store().addWaypointAt(48.66, -123.39)
  eq(store().redoStack.length, 0, 'redo branch cleared')
  eq(store().waypoints.length, 1, 'the new waypoint is the plan')
})

await check('undo of a removal restores the waypoint at its index', () => {
  reset()
  store().addWaypointAt(48.6493, -123.3982)
  store().addWaypointAt(48.65, -123.397)
  store().addWaypointAt(48.651, -123.396)
  const second = store().waypoints[1]
  store().removeWaypoint(1)
  eq(store().waypoints.length, 2, 'removed')
  store().undo()
  eq(store().waypoints.length, 3, 'undo restores three')
  eq(store().waypoints[1].position.latitude_deg, second.position.latitude_deg, 'same index')
})

await check('undo of clearPlan restores waypoints, home and blocks', () => {
  reset()
  store().addWaypointAt(48.6493, -123.3982)
  store().clearPlan()
  eq(store().waypoints.length, 0, 'cleared')
  store().undo()
  eq(store().waypoints.length, 1, 'plan restored')
})

await check('undo of an inserted pattern removes the whole block', () => {
  reset()
  store().addWaypointAt(48.6493, -123.3982)
  store().insertPattern(
    {
      waypoints: [
        {
          position: { latitude_deg: 48.65, longitude_deg: -123.39 },
          altitude: { datum: 'AMSL_EGM96', meters: 60 },
          command: 16,
          params: [0, 0, 0, 0],
        },
        {
          position: { latitude_deg: 48.651, longitude_deg: -123.391 },
          altitude: { datum: 'AMSL_EGM96', meters: 60 },
          command: 16,
          params: [0, 0, 0, 0],
        },
      ],
      lines: [],
    },
    'test sweep',
    { latitude_deg: 48.65, longitude_deg: -123.39 },
  )
  eq(store().waypoints.length, 3, 'hand waypoint + block')
  eq(store().lastPattern?.count, 2, 'pattern remembered')
  store().undo()
  eq(store().waypoints.length, 1, 'undo drops the whole block')
  eq(store().lastPattern, null, 'pattern memory restored to empty')
})

await check('a plan fetched from the FC resets history', () => {
  reset()
  store().addWaypointAt(48.6493, -123.3982)
  assert(store().undoStack.length > 0, 'history present before download')
  store().handlePlan([
    {
      command: 16,
      frame: 6,
      current: false,
      autocontinue: true,
      params: [0, 0, 0, 0],
      x: 486500000,
      y: -1233970000,
      z: 80,
      seq: 0,
    },
  ])
  eq(store().undoStack.length, 0, 'undo stack cleared')
  eq(store().redoStack.length, 0, 'redo stack cleared')
  store().undo()
  eq(store().waypoints.length, 1, 'undo cannot resurrect the pre-download plan')
})

await check('a batch delete is one undo step and keeps the unselected rows', () => {
  reset()
  store().addWaypointAt(48.6493, -123.3982) // 0
  store().addWaypointAt(48.65, -123.397) // 1
  store().addWaypointAt(48.651, -123.396) // 2
  const middle = store().waypoints[1]
  const before = store().undoStack.length

  store().removeWaypoints([0, 2])
  eq(store().waypoints.length, 1, 'two removed at once')
  eq(store().undoStack.length, before + 1, 'a single history entry')
  eq(store().waypoints[0].position.latitude_deg, middle.position.latitude_deg, 'row 1 kept')

  store().undo()
  eq(store().waypoints.length, 3, 'undo restores the whole selection')
  eq(store().undoStack.length, before, 'back to the pre-batch stack')
})

await check('an empty batch delete is a no-op (no history entry)', () => {
  reset()
  store().addWaypointAt(48.6493, -123.3982)
  const before = store().undoStack.length
  store().removeWaypoints([])
  eq(store().waypoints.length, 1, 'plan unchanged')
  eq(store().undoStack.length, before, 'no history pushed')
})

await check('a batch altitude set is one undo step', () => {
  reset()
  store().addWaypointAt(48.6493, -123.3982)
  store().addWaypointAt(48.65, -123.397)
  store().addWaypointAt(48.651, -123.396)
  const before = store().undoStack.length

  store().setAltitudeMany([0, 2], 123)
  eq(store().waypoints[0].altitude.meters, 123, 'row 0 set')
  eq(store().waypoints[1].altitude.meters !== 123, true, 'row 1 untouched')
  eq(store().waypoints[2].altitude.meters, 123, 'row 2 set')
  eq(store().undoStack.length, before + 1, 'a single history entry')

  store().undo()
  eq(store().waypoints[0].altitude.meters !== 123, true, 'undo restores row 0')
  eq(store().waypoints[2].altitude.meters !== 123, true, 'undo restores row 2')
})

await check('a batch edit after a generate releases the pattern block', () => {
  reset()
  store().insertPattern(
    {
      waypoints: [
        {
          position: { latitude_deg: 48.65, longitude_deg: -123.39 },
          altitude: { datum: 'AMSL_EGM96', meters: 60 },
          command: 16,
          params: [0, 0, 0, 0],
        },
        {
          position: { latitude_deg: 48.651, longitude_deg: -123.391 },
          altitude: { datum: 'AMSL_EGM96', meters: 60 },
          command: 16,
          params: [0, 0, 0, 0],
        },
      ],
      lines: [],
    },
    'test sweep',
    { latitude_deg: 48.65, longitude_deg: -123.39 },
  )
  assert(store().lastPattern !== null, 'pattern remembered')
  store().removeWaypoints([0])
  eq(store().lastPattern, null, 'block released by the batch edit')
})

console.log('plan undo/redo:')
if (failures.length > 0) {
  console.error(`\n${failures.length} check(s) failed:`)
  for (const f of failures) console.error(`  - ${f}`)
  process.exit(1)
}
console.log(`all ${passed} checks passed`)
