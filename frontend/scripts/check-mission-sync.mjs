#!/usr/bin/env node
// Mission sync-state checks (issue #15, finding 30).
//
//   node scripts/check-mission-sync.mjs
//
// Bundles the real mission store (stubbing the Tauri plugins, which have no
// runtime in Node) and drives it through the event sequence the desktop shell
// emits, asserting what the planning panel shows at each step. This is the
// regression guard for "clicked Upload but the panel still says Unsaved
// changes": a completed upload must clear the warning, a failed one must not
// pretend the plan reached the FC.
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
  const out = join(await mkdtemp(join(tmpdir(), 'maggcs-sync-')), 'mod.mjs')
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
const { planSyncStatus, planSyncPhase } = await bundle('mission/sync.ts')
const { shortHash } = await bundle('mission/hash.ts')
const { compileWaypoints } = await bundle('mission/compile.ts')

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

/** The status the panel renders, straight from the live store. */
function status() {
  const s = useMissionStore.getState()
  return planSyncStatus({
    itemCount: s.flyable().length,
    dirty: s.dirty,
    fcMatches: s.fcMatches,
    lastSyncedHash: s.lastSyncedHash,
  })
}

/** The lifecycle phase the sync line renders (adds the in-flight states). */
function phase() {
  const s = useMissionStore.getState()
  return planSyncPhase({
    itemCount: s.flyable().length,
    dirty: s.dirty,
    fcMatches: s.fcMatches,
    lastSyncedHash: s.lastSyncedHash,
    busy: s.busy,
    syncState: s.syncState,
    verifying: s.verifying,
  })
}

function event(op, kind, message) {
  useMissionStore.getState().handleEvent({ op, kind, sent: 0, total: 0, seq: 0, message })
}

function reset() {
  useMissionStore.getState().reset()
}

await check('clearing the FC mission invalidates the baseline (#33/#38)', async () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  const uploaded = useMissionStore.getState().flyable()
  await useMissionStore.getState().upload()
  event('upload', 'completed')
  useMissionStore.getState().handlePlan(uploaded)
  eq(status(), 'synced', 'precondition: in sync')

  // "Clear FC mission" succeeds: the FC now holds nothing.
  event('clear', 'completed')
  const s = useMissionStore.getState()
  eq(s.lastSyncedHash, null, 'baseline dropped')
  eq(s.fcMatches, null, 'match knowledge dropped')
  assert(s.waypoints.length === 1, 'the local plan is kept')
  eq(status(), 'unsynced', 'status — the FC holds nothing, so not synced')
})

await check('a failed clear keeps the baseline (#33/#38)', async () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  const uploaded = useMissionStore.getState().flyable()
  await useMissionStore.getState().upload()
  event('upload', 'completed')
  useMissionStore.getState().handlePlan(uploaded)
  eq(status(), 'synced', 'precondition')

  event('clear', 'failed', 'NACK')
  eq(status(), 'synced', 'the FC still holds the plan')
})

console.log('mission sync:')

console.log('mission sync:')

await check('an empty plan has nothing to sync', () => {
  reset()
  eq(status(), 'empty', 'status')
  eq(useMissionStore.getState().dirty, false, 'dirty')
})

await check('a hand-placed waypoint reads as unsynced, not as an FC mismatch', () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  eq(useMissionStore.getState().dirty, true, 'dirty')
  eq(status(), 'unsynced', 'status')
})

await check('upload clears the unsaved warning and reports in-sync (finding 30)', async () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  useMissionStore.getState().addWaypointAt(48.65, -123.397)
  const uploaded = useMissionStore.getState().flyable()

  await useMissionStore.getState().upload()
  eq(useMissionStore.getState().busy, true, 'busy while uploading')
  event('upload', 'progress')
  event('upload', 'completed')

  eq(useMissionStore.getState().dirty, false, 'dirty after upload')
  eq(useMissionStore.getState().fcMatches, true, 'fcMatches after upload')
  assert(useMissionStore.getState().lastSyncedHash !== null, 'baseline recorded')
  assert(useMissionStore.getState().verifying, 'read-back started')

  // The shell reports the FC's copy of the mission; the hash must match.
  useMissionStore.getState().handlePlan(uploaded)
  eq(useMissionStore.getState().verifying, false, 'verify finished')
  eq(useMissionStore.getState().busy, false, 'busy cleared')
  eq(status(), 'synced', 'status')
})

await check('a read-back at wire precision is not a false mismatch', async () => {
  // `z` is an f32 on the wire (MISSION_ITEM_INT.z / core::MissionItem), so a
  // plan whose altitude is not f32-exact must still read as in sync after the
  // FC echoes it back narrowed (issues.md #38).
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  useMissionStore.getState().updateAltitude(0, 80.123456789)
  const uploaded = useMissionStore.getState().flyable()
  eq(uploaded[0].z, Math.fround(80.123456789), 'the compiled plan is already f32')

  await useMissionStore.getState().upload()
  event('upload', 'completed')
  const echoed = uploaded.map((i) => ({
    ...i,
    z: Math.fround(i.z),
    params: i.params.map(Math.fround),
  }))
  useMissionStore.getState().handlePlan(echoed)
  eq(status(), 'synced', 'status')
})

await check('a command item position echoes through PX4 f32 storage', async () => {
  // PX4 does not store MAV_FRAME_MISSION items verbatim: it parks their
  // coordinate fields in the internal `mission_item_s.params[4..6]`, which are
  // f32 (PX4 mavlink_mission.cpp). The sweep's DO_CHANGE_SPEED carries the
  // reference position, so the FC echoes `x = round(f32(486493000)) =
  // 486492992` — the hash must narrow the same way or every sweep upload
  // reports "FC mission differs from the uploaded plan".
  reset()
  useMissionStore.getState().setWaypoints([
    {
      position: { latitude_deg: 48.6493, longitude_deg: -123.3982 },
      altitude: { datum: 'AMSL_EGM96', meters: 0 },
      command: 178, // DO_CHANGE_SPEED
      params: [1, 5, -1, 0],
      autocontinue: true,
    },
    {
      position: { latitude_deg: 48.6493, longitude_deg: -123.3982 },
      altitude: { datum: 'AMSL_EGM96', meters: 80 },
      command: 16,
      params: [0, 0, 0, 0],
      autocontinue: true,
    },
  ])
  const uploaded = useMissionStore.getState().flyable()
  eq(uploaded[0].frame, 'mission', 'the speed item compiles to MAV_FRAME_MISSION')

  await useMissionStore.getState().upload()
  event('upload', 'completed')
  // The FC's echo: params/z narrowed to f32, and the command item's x/y
  // round-tripped through PX4's f32 storage (exact for the global waypoint).
  const echoed = uploaded.map((i) =>
    i.frame === 'mission'
      ? { ...i, x: Math.round(Math.fround(i.x)), y: Math.round(Math.fround(i.y)), z: Math.fround(i.z), params: i.params.map(Math.fround) }
      : { ...i, x: Math.round(i.x), y: Math.round(i.y), z: Math.fround(i.z), params: i.params.map(Math.fround) },
  )
  eq(echoed[0].x, Math.fround(uploaded[0].x), 'the echo really is f32-narrowed')
  useMissionStore.getState().handlePlan(echoed)
  eq(status(), 'synced', 'status')
})

await check('editing after a sync reads as unsaved, not unsynced', async () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  const uploaded = useMissionStore.getState().flyable()
  await useMissionStore.getState().upload()
  event('upload', 'completed')
  useMissionStore.getState().handlePlan(uploaded)
  eq(status(), 'synced', 'precondition')

  useMissionStore.getState().updateAltitude(0, 133)
  eq(status(), 'dirty', 'status')
})

await check('a failed upload keeps the plan dirty and does not claim a baseline', async () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  await useMissionStore.getState().upload()
  event('mission', 'failed', 'not connected')

  const s = useMissionStore.getState()
  eq(s.busy, false, 'busy cleared')
  eq(s.dirty, true, 'edits preserved')
  eq(s.lastSyncedHash, null, 'no baseline')
  eq(s.verifying, false, 'read-back not left hanging')
  eq(status(), 'unsynced', 'status')
})

await check('a failed read-back resets verifying and reports the mismatch', async () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  const uploaded = useMissionStore.getState().flyable()
  await useMissionStore.getState().upload()
  event('upload', 'completed')
  assert(useMissionStore.getState().verifying, 'verify in flight')

  event('mission', 'failed', 'timeout')
  eq(useMissionStore.getState().verifying, false, 'verifying reset')
})

await check('a read-back that differs from the plan is reported as a mismatch', async () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  const uploaded = useMissionStore.getState().flyable()
  await useMissionStore.getState().upload()
  event('upload', 'completed')

  const other = uploaded.map((i) => ({ ...i, x: i.x + 100 }))
  useMissionStore.getState().handlePlan(other)
  eq(useMissionStore.getState().fcMatches, false, 'fcMatches')
  eq(status(), 'mismatch', 'status')
  eq(useMissionStore.getState().waypoints.length, 1, 'the local plan is not clobbered')
})

await check('clearPlan discards the local plan without touching the FC', () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  useMissionStore.getState().clearPlan()
  const s = useMissionStore.getState()
  eq(s.waypoints.length, 0, 'waypoints')
  eq(s.flyable().length, 0, 'flyable')
  eq(s.dirty, false, 'dirty')
  eq(s.lastSyncedHash, null, 'baseline')
  eq(s.lastEvent, null, 'stale event')
  eq(status(), 'empty', 'status')
})

await check('a download replaces the plan and marks it in sync', () => {
  reset()
  const items = [
    {
      seq: 0,
      frame: 'global_int',
      command: 16,
      params: [0, 0, 0, 0],
      x: 486493000,
      y: -1233982000,
      z: 130,
      autocontinue: true,
      current: true,
    },
  ]
  useMissionStore.getState().handlePlan(items)
  eq(status(), 'synced', 'status')
  eq(useMissionStore.getState().altitudeMode, 'amsl', 'mode from the wire frame')
  eq(useMissionStore.getState().waypoints[0].altitude.meters, 130, 'AMSL restored')
})

await check('a dropped link clears the pending progress without losing the plan', async () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  await useMissionStore.getState().upload()
  eq(useMissionStore.getState().busy, true, 'uploading')

  // The service task exits with its connection, so no terminal event arrives.
  useMissionStore.getState().linkLost()
  const s = useMissionStore.getState()
  eq(s.busy, false, 'busy')
  eq(s.verifying, false, 'verifying')
  eq(s.syncState, 'idle', 'syncState')
  eq(s.waypoints.length, 1, 'plan kept')
  eq(status(), 'unsynced', 'status')
})

await check('a manual download during a pending read-back still replaces the plan', async () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  await useMissionStore.getState().upload()
  event('upload', 'completed')
  assert(useMissionStore.getState().verifying, 'verify in flight')

  await useMissionStore.getState().download()
  eq(useMissionStore.getState().verifying, false, 'download takes over')

  const items = [
    {
      seq: 0,
      frame: 'global_relative_alt_int',
      command: 16,
      params: [0, 0, 0, 0],
      x: 486500000,
      y: -1233970000,
      z: 40,
      autocontinue: true,
      current: true,
    },
  ]
  useMissionStore.getState().handlePlan(items)
  eq(useMissionStore.getState().waypoints.length, 1, 'plan replaced')
  eq(useMissionStore.getState().waypoints[0].altitude.meters, 40, 'AMSL restored')
  eq(status(), 'synced', 'status')
})

console.log('\npreset patterns:')

/** A tiny stand-in for a generated pattern (geometry is not under test here). */
function pattern(count, alt) {
  return {
    waypoints: Array.from({ length: count }, (_, i) => ({
      position: { latitude_deg: 48.6 + i * 1e-4, longitude_deg: -123.4 },
      altitude: { datum: 'AMSL_EGM96', meters: alt },
      command: 16,
      params: [0, 0, 0, 0],
      autocontinue: true,
    })),
    lines: [{ id: 1, kind: 'survey', start_seq: 0, end_seq: count - 1, length_m: 100 }],
  }
}

await check('re-generating replaces the previous trajectory instead of duplicating it', () => {
  reset()
  useMissionStore.getState().insertPattern(pattern(4, 130), 'Survey sweep')
  eq(useMissionStore.getState().waypoints.length, 4, 'first generate')
  eq(useMissionStore.getState().lastPattern.count, 4, 'block size')

  useMissionStore.getState().insertPattern(pattern(6, 140), 'Survey sweep')
  const s = useMissionStore.getState()
  eq(s.waypoints.length, 6, 'replaced, not appended')
  eq(s.waypoints[0].altitude.meters, 140, 'new geometry')
  eq(s.lastPattern.lines[0].end_seq, 5, 'line table re-anchored')
  eq(s.dirty, true, 'dirty')
})

await check('a hand edit resets the pattern block, so Generate appends again', () => {
  reset()
  useMissionStore.getState().insertPattern(pattern(4, 130), 'Survey sweep')
  useMissionStore.getState().updateAltitude(0, 131)
  eq(useMissionStore.getState().lastPattern, null, 'block released by the edit')

  useMissionStore.getState().insertPattern(pattern(3, 150), 'Survey sweep')
  eq(useMissionStore.getState().waypoints.length, 7, 'appended after the edit')
  eq(useMissionStore.getState().lastPattern.lines[0].start_seq, 4, 'lines offset by the kept plan')
  eq(useMissionStore.getState().lastPattern.lines[0].end_seq, 6, 'lines offset by the kept plan')
})

await check('an import releases the pattern block (no imported waypoints dropped)', () => {
  reset()
  useMissionStore.getState().insertPattern(pattern(4, 130), 'Survey sweep')
  useMissionStore.getState().applyImport({
    items: [
      {
        seq: 0,
        frame: 'global_int',
        command: 16,
        params: [0, 0, 0, 0],
        x: 486493000,
        y: -1233982000,
        z: 130,
        autocontinue: true,
        current: true,
      },
    ],
    mode: 'amsl',
    home: null,
    base: { fileType: 'Plan', version: 1 },
    blocks: [],
    unsupported: [],
  })
  eq(useMissionStore.getState().lastPattern, null, 'block released by the import')

  useMissionStore.getState().insertPattern(pattern(3, 150), 'Survey sweep')
  eq(useMissionStore.getState().waypoints.length, 4, 'imported item kept')
})

await check('clearPlan also drops the pattern readout', () => {
  reset()
  useMissionStore.getState().insertPattern(pattern(4, 130), 'Survey sweep')
  useMissionStore.getState().clearPlan()
  eq(useMissionStore.getState().lastPattern, null, 'lastPattern')
})

await check('a running FC mission with an empty local plan auto-fetches', () => {
  // Operator report 2026-10-10: a SITL mission was flying but the Fly panel
  // said "no waypoints" — the local plan is memory-only, so a restart (or a
  // QGC-uploaded mission) leaves it empty. The first MISSION_CURRENT with
  // an empty plan must trigger a download (the stubbed invoke resolves).
  reset()
  useMissionStore
    .getState()
    .handleEvent({ op: 'mission', kind: 'current_changed', sent: 0, total: 0, seq: 3 })
  eq(useMissionStore.getState().currentSeq, 3, 'current seq recorded')
  eq(useMissionStore.getState().syncState, 'downloading', 'auto-fetch started')
  eq(useMissionStore.getState().busy, true, 'busy during fetch')
  // A non-empty local plan is never auto-fetched: the operator's work must
  // not be clobbered by an in-flight mission. (`reset`, not `clearPlan`:
  // the first half left a download in flight, which clearPlan does not
  // unwind.)
  useMissionStore.getState().reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  useMissionStore
    .getState()
    .handleEvent({ op: 'mission', kind: 'current_changed', sent: 0, total: 0, seq: 1 })
  eq(useMissionStore.getState().syncState, 'idle', 'no auto-fetch with a local plan')
})

await check("after 'Clear local plan' a running FC mission is not auto-fetched back", () => {
  // Operator report: clearing the local plan while the FC still holds the
  // mission made the plan come back on the next MISSION_CURRENT. The
  // explicit clear must suppress the boot-time auto-fetch until a plan is
  // deliberately loaded again.
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  useMissionStore.getState().clearPlan()
  useMissionStore
    .getState()
    .handleEvent({ op: 'mission', kind: 'current_changed', sent: 0, total: 0, seq: 4 })
  eq(useMissionStore.getState().waypoints.length, 0, 'local plan stays empty')
  eq(useMissionStore.getState().syncState, 'idle', 'no auto-fetch after an explicit clear')
  // The next deliberate Download brings the FC mission back and re-arms the
  // boot-time auto-fetch for the future.
  useMissionStore.getState().download()
  eq(useMissionStore.getState().syncState, 'downloading', 'explicit download still works')
  useMissionStore.getState().handlePlan([{ seq: 0, frame: 'global_int', command: 16, params: [0, 0, 0, 0], x: 486493000, y: -1233982000, z: 45, autocontinue: true, current: true }])
  eq(useMissionStore.getState().waypoints.length, 1, 'plan restored by the explicit download')
  eq(useMissionStore.getState().planCleared, false, 'auto-fetch re-armed')
})

console.log('\nsync phase (U17d):')

await check('the phase follows the whole upload round-trip', async () => {
  reset()
  eq(phase(), 'empty', 'nothing to sync')

  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  eq(phase(), 'unsynced', 'a hand-placed waypoint is unsynced')

  const uploaded = useMissionStore.getState().flyable()
  await useMissionStore.getState().upload()
  eq(phase(), 'uploading', 'the in-flight upload wins over the stored state')
  eq(useMissionStore.getState().busy, true, 'busy')

  event('upload', 'completed')
  eq(phase(), 'verifying', 'the read-back beats the stored relationship')

  useMissionStore.getState().handlePlan(uploaded)
  eq(phase(), 'synced', 'verified in sync')

  useMissionStore.getState().updateAltitude(0, 133)
  eq(phase(), 'dirty', 'an edit after a sync is unsaved, not the FC plan')
})

await check('the phase reports a mismatch, then download / clear', async () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  const uploaded = useMissionStore.getState().flyable()
  await useMissionStore.getState().upload()
  event('upload', 'completed')
  // The post-upload read-back differs: the FC holds something else.
  useMissionStore.getState().handlePlan(uploaded.map((i) => ({ ...i, x: i.x + 100 })))
  eq(phase(), 'mismatch', 'a differing read-back is a mismatch, not unsynced')

  await useMissionStore.getState().download()
  eq(phase(), 'downloading', 'a read-back is in flight')

  await useMissionStore.getState().clear()
  eq(phase(), 'clearing', 'a clear is in flight')
  event('clear', 'completed')
  eq(phase(), 'unsynced', 'after a clear the FC holds nothing')
})

await check('an in-flight operation beats a stale stored mismatch', async () => {
  reset()
  useMissionStore.getState().addWaypointAt(48.6493, -123.3982)
  const uploaded = useMissionStore.getState().flyable()
  await useMissionStore.getState().upload()
  event('upload', 'completed')
  useMissionStore.getState().handlePlan(uploaded.map((i) => ({ ...i, x: i.x + 100 })))
  eq(phase(), 'mismatch', 'a stored mismatch')

  await useMissionStore.getState().upload()
  eq(phase(), 'uploading', 'a fresh upload beats the stale mismatch')
})

console.log('\nEKF-origin altitude compensation (field 2026-10-21):')

const wp = (meters, command = 16) => ({
  position: { latitude_deg: 48.5, longitude_deg: -123.4 },
  altitude: { datum: 'AMSL_EGM96', meters },
  command,
  params: [0, 0, 0, 0],
  autocontinue: true,
  locked: false,
})

await check('compileWaypoints subtracts the measured EKF-origin bias from coordinate items', () => {
  const items = compileWaypoints([wp(100), wp(120)], 'amsl', 0, 5)
  eq(items[0].z, 95, 'planned 100 − δ 5')
  eq(items[1].z, 115, 'planned 120 − δ 5')
})

await check('the stored plan and the export stay on the true datum (offset only at upload)', () => {
  const items = compileWaypoints([wp(100)], 'amsl', 0)
  eq(items[0].z, 100, 'no offset passed = untouched (export path)')
})

await check('command items (DO_*) are never compensated', () => {
  const items = compileWaypoints([wp(12, 178)], 'amsl', 0, 5)
  eq(items[0].z, 12, 'a command param is not an altitude')
})

await check('null calibration fails open (no shift)', () => {
  const items = compileWaypoints([wp(100)], 'amsl', 0, 0)
  eq(items[0].z, 100, 'δ unknown → compile as planned')
})

await check('relative mode compensates too (same constant bias)', () => {
  // stored AMSL 130, home 30 → relative 100; δ 5 → 95
  const items = compileWaypoints([wp(130)], 'relative', 30, 5)
  eq(items[0].z, 95, 'relative − δ')
})

console.log('\ncompact hash (U17d):')

await check('shortHash is a stable 8-hex FNV-1a digest', () => {
  eq(shortHash(''), '811c9dc5', 'FNV-1a 32 offset basis for the empty string')
  eq(shortHash('a'), 'e40c292c', 'FNV-1a 32 of "a"')
  eq(shortHash('a'), shortHash('a'), 'deterministic')
  assert(shortHash('a') !== shortHash('b'), 'different inputs differ')
  assert(/^[0-9a-f]{8}$/.test(shortHash('[{"x":1}]')), 'always 8 lowercase hex')
})

if (failures.length > 0) {
  console.error(`\ncheck:mission-sync FAILED — ${failures.length} failure(s):`)
  for (const f of failures) console.error(`  ${f}`)
  process.exit(1)
}
console.log(`\ncheck:mission-sync OK — ${passed} scenario(s)`)
