#!/usr/bin/env node
// Real-store owned Mission regression: native replies, snapshot identity, cancellation and target isolation.
import assert from 'node:assert/strict'
import { missionFixture, tick, waitUntil } from './mission-store-fixture.mjs'

const h = await missionFixture()
const state = () => h.store.getState()
const status = () => h.planSyncStatus({ itemCount: state().flyable().length, dirty: state().dirty,
  fcMatches: state().fcMatches, lastSyncedHash: state().lastSyncedHash })
const phase = () => h.planSyncPhase({ itemCount: state().flyable().length, dirty: state().dirty,
  fcMatches: state().fcMatches, lastSyncedHash: state().lastSyncedHash,
  busy: state().busy, syncState: state().syncState, verifying: state().verifying })
const pending = (command) => h.pending.find((entry) => entry.command === command)
async function beginUpload() {
  const promise = state().upload()
  await waitUntil(() => Boolean(pending('mission_upload')))
  return { promise, request: pending('mission_upload') }
}
async function sync() {
  const upload = await beginUpload()
  h.resolve('mission_upload')
  await upload.promise
  await waitUntil(() => Boolean(pending('mission_download')))
  h.resolve('mission_download')
  await waitUntil(() => !state().busy)
}
const failures = []
let passed = 0
async function check(name, work) {
  await h.reset()
  try { await work(); passed += 1; console.log(`PASS ${name}`) }
  catch (error) { failures.push(`${name}: ${error.stack}`); console.error(`FAIL ${name}: ${error.message}`) }
}

await check('empty plan sends no upload and has no baseline', async () => {
  await state().upload()
  assert.equal(status(), 'empty')
  assert.equal(h.pending.length, 0)
})
await check('new waypoint is unsynced, and accepted upload waits for matching read-back', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  assert.equal(status(), 'unsynced')
  const upload = await beginUpload()
  assert.equal(phase(), 'uploading')
  h.resolve('mission_upload')
  await upload.promise
  await waitUntil(() => Boolean(pending('mission_download')))
  assert.equal(state().fcMatches, null)
  assert.equal(phase(), 'verifying')
  h.resolve('mission_download')
  await waitUntil(() => !state().busy)
  assert.equal(status(), 'synced')
})
await check('progress/terminal publications require the original request and owner', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const upload = await beginUpload()
  h.progress(upload.request, 1, 2)
  assert.equal(state().lastEvent.sent, 1)
  h.progress(upload.request, 9, 9, { request_id: 'foreign-request' })
  h.progress(upload.request, 9, 9, { operation: { ...upload.request.operation, owner: { client_id: 'foreign', instance_id: 'foreign' } } })
  assert.equal(state().lastEvent.sent, 1)
  state().handleOperation({ operation: upload.request.operation, request_id: 'foreign-request', phase: 'finished', outcome: { kind: 'succeeded' } })
  assert.equal(state().busy, true)
  h.reject('mission_upload', { code: 'protocol', error: { code: 'ack_denied', ack_type: 2 } })
  await upload.promise
  assert.equal(state().dirty, true)
  assert.equal(state().lastSyncedHash, null)
})
await check('wire precision and command-item f32 storage compare without false mismatch', async () => {
  state().setWaypoints([
    { position: { latitude_deg: 48.6493, longitude_deg: -123.3982 }, altitude: { datum: 'AMSL_EGM96', meters: 0 }, command: 178, params: [1, 5, -1, 0], autocontinue: true, locked: false },
    { position: { latitude_deg: 48.6493, longitude_deg: -123.3982 }, altitude: { datum: 'AMSL_EGM96', meters: 80.123456789 }, command: 16, params: [0, 0, 0, 0], autocontinue: true, locked: false },
  ])
  const upload = await beginUpload()
  assert.equal(upload.request.input.items[1].z, Math.fround(80.123456789))
  h.resolve('mission_upload'); await upload.promise
  await waitUntil(() => Boolean(pending('mission_download')))
  h.resolve('mission_download', { kind: 'downloaded', items: h.fcItems.map((item) => item.frame === 'mission' ? { ...item, x: Math.fround(item.x), y: Math.fround(item.y) } : item) })
  await waitUntil(() => !state().busy)
  assert.equal(status(), 'synced')
})
await check('editing during upload cannot change the sent snapshot or verify the later editor', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const upload = await beginUpload()
  const sentHash = h.itemsHash(upload.request.input.items)
  const sentZ = upload.request.input.items[0].z
  state().updateAltitude(0, 133)
  assert.equal(upload.request.input.items[0].z, sentZ)
  h.resolve('mission_upload'); await upload.promise
  await waitUntil(() => Boolean(pending('mission_download')))
  h.resolve('mission_download')
  await waitUntil(() => !state().busy)
  assert.equal(state().lastSyncedHash, sentHash)
  assert.equal(state().waypoints[0].altitude.meters, 133)
  assert.equal(state().dirty, true)
  assert.notEqual(state().fcMatches, true)
  assert.equal(status(), 'dirty')
})
await check('read-back mismatch is retained without overwriting the local draft', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const upload = await beginUpload()
  h.resolve('mission_upload'); await upload.promise
  await waitUntil(() => Boolean(pending('mission_download')))
  h.resolve('mission_download', { kind: 'downloaded', items: h.fcItems.map((item) => ({ ...item, x: item.x + 100 })) })
  await waitUntil(() => !state().busy)
  assert.equal(status(), 'mismatch')
  assert.equal(state().waypoints[0].position.latitude_deg, 48.6493)
})
await check('failed read-back cannot leave a verified baseline or stuck busy state', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const upload = await beginUpload()
  h.resolve('mission_upload'); await upload.promise
  await waitUntil(() => Boolean(pending('mission_download')))
  h.reject('mission_download', { code: 'reconciliation_required', reason: 'Read-back outcome is unknown' })
  await waitUntil(() => !state().busy)
  assert.equal(state().verifying, false)
  assert.equal(state().fcMatches, null)
  assert.notEqual(status(), 'synced')
  assert.equal(state().requiresReconciliation, true)
})
await check('clear FC mission removes its baseline but retains the local plan', async () => {
  state().addWaypointAt(48.6493, -123.3982); await sync()
  const promise = state().clear()
  await waitUntil(() => Boolean(pending('mission_clear')))
  h.resolve('mission_clear'); await promise
  assert.equal(state().waypoints.length, 1)
  assert.equal(state().lastSyncedHash, null)
  assert.equal(state().fcMatches, null)
  assert.equal(status(), 'unsynced')
})
await check('an unsent busy refusal preserves the existing confirmed baseline', async () => {
  state().addWaypointAt(48.6493, -123.3982); await sync()
  const promise = state().clear()
  await waitUntil(() => Boolean(pending('mission_clear')))
  h.reject('mission_clear', { code: 'operation', error: { code: 'resource_busy' } }); await promise
  assert.equal(status(), 'synced')
})
await check('local clear during upload is not repopulated by automatic read-back', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const upload = await beginUpload()
  state().clearPlan()
  h.resolve('mission_upload'); await upload.promise; await tick()
  assert.equal(state().waypoints.length, 0)
  assert.equal(state().planCleared, true)
  assert.equal(pending('mission_download'), undefined)
})
await check('explicit download resets history and preserves nullable MAVLink parameters', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const promise = state().download()
  await waitUntil(() => Boolean(pending('mission_download')))
  const item = { seq: 0, frame: 'global_int', command: 16, params: [0, 0, 0, null], x: 486500000, y: -1233970000, z: 80, autocontinue: true, current: true }
  h.resolve('mission_download', { kind: 'downloaded', items: [item] }); await promise
  assert.equal(state().undoStack.length, 0)
  assert.equal(state().waypoints[0].altitude.meters, 80)
  assert(Number.isNaN(state().waypoints[0].params[3]))
  assert.equal(h.itemsHash(state().flyable()), h.itemsHash([item]))
  const upload = await beginUpload()
  assert.equal(upload.request.input.items[0].params[3], null)
  h.reject('mission_upload', { code: 'operation', error: { code: 'resource_busy' } }); await upload.promise
})
await check('download completion cannot overwrite edits made while waiting', async () => {
  const promise = state().download()
  await waitUntil(() => Boolean(pending('mission_download')))
  state().addWaypointAt(48.7, -123.4)
  h.resolve('mission_download', { kind: 'downloaded', items: [] }); await promise
  assert.equal(state().waypoints.length, 1)
  assert.equal(state().waypoints[0].position.latitude_deg, 48.7)
  assert.match(state().lastEvent.message, /Local plan changed/)
})
await check('a new download cannot replace another active owned read-back', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const upload = await beginUpload()
  h.resolve('mission_upload'); await upload.promise
  await waitUntil(() => Boolean(pending('mission_download')))
  const count = h.requests.filter((request) => request.command === 'mission_download').length
  await state().download()
  assert.equal(h.requests.filter((request) => request.command === 'mission_download').length, count)
  h.resolve('mission_download'); await waitUntil(() => !state().busy)
})
await check('cancel uses the exact current session/operation and retains uncertain FC state', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const upload = await beginUpload()
  await waitUntil(() => state().operation !== null)
  await state().cancel(); await upload.promise
  assert.equal(h.cancelCalls.length, 1)
  assert.deepEqual(h.cancelCalls[0].operation, upload.request.operation)
  assert.deepEqual(h.cancelCalls[0].session, upload.request.context.session)
  assert.equal(state().requiresReconciliation, true)
  assert.equal(state().busy, false)
  assert.equal(state().waypoints.length, 1)
})
await check('cancel before target attachment produces zero effect RPCs', async () => {
  h.deferTargets = true
  state().addWaypointAt(48.6493, -123.3982)
  const promise = state().upload()
  await waitUntil(() => h.targetsWaiters.length > 0)
  await state().cancel()
  for (const release of h.targetsWaiters.splice(0)) release()
  h.deferTargets = false
  await promise
  assert.equal(h.pending.length, 0)
  assert.equal(state().busy, false)
  assert.match(state().lastEvent.message, /before dispatch/)
})
await check('target loss clears only FC authority and rejects an old accepted reply', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const upload = await beginUpload()
  state().linkLost(); h.revision += 1
  h.resolve('mission_upload'); await upload.promise
  assert.equal(state().waypoints.length, 1)
  assert.equal(state().lastSyncedHash, null)
  assert.equal(state().busy, false)
  assert.equal(state().operation, null)
})
await check('reconnect re-reads a retained local plan before restoring FC sync authority', async () => {
  state().addWaypointAt(48.6493, -123.3982); await sync()
  state().linkLost()
  assert.equal(state().lastSyncedHash, null)
  state().linkRestored()
  await waitUntil(() => Boolean(pending('mission_download')))
  h.resolve('mission_download')
  await waitUntil(() => !state().busy)
  assert.equal(state().fcMatches, true)
  assert.notEqual(state().lastSyncedHash, null)
})
await check('a silently replaced native generation cannot establish reply acceptance', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const upload = await beginUpload()
  h.revision += 1
  h.resolve('mission_upload'); await upload.promise
  assert.equal(state().lastSyncedHash, null)
  assert.equal(state().fcMatches, null)
  assert.equal(state().busy, false)
})
await check('a wrong reply owner cannot establish a verified result', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const upload = await beginUpload()
  h.resolve('mission_upload', null, { ...upload.request.operation, owner: { client_id: 'foreign', instance_id: 'foreign' } })
  await upload.promise
  assert.equal(state().lastSyncedHash, null)
  assert.match(state().lastEvent.message, /does not match/)
})
await check('current reply requires the requested sequence and passive observation remains separate', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const promise = state().setCurrent(3)
  await waitUntil(() => Boolean(pending('mission_set_current')))
  h.current(2); await tick(); await tick()
  assert.equal(state().busy, true)
  h.resolve('mission_set_current', { kind: 'current', seq: 2 }); await promise
  assert.match(state().lastEvent.message, /did not confirm/)
})
await check('newer passive current observation is not rolled back by a delayed command reply', async () => {
  state().addWaypointAt(48.6493, -123.3982)
  const promise = state().setCurrent(3)
  await waitUntil(() => Boolean(pending('mission_set_current')))
  h.current(4, h.target(), Date.now() + 10)
  await waitUntil(() => state().currentSeq === 4)
  h.resolve('mission_set_current'); await promise
  assert.equal(state().currentSeq, 4)
})
await check('passive current observation fetches only an empty non-cleared local plan', async () => {
  h.current(3)
  await waitUntil(() => Boolean(pending('mission_download')))
  assert.equal(state().currentSeq, 3)
  h.resolve('mission_download'); await waitUntil(() => !state().busy)
  state().clearPlan()
  h.current(4, h.target(), Date.now() + 10); await tick(); await tick()
  assert.equal(pending('mission_download'), undefined)
  assert.equal(state().waypoints.length, 0)
})
await check('all effect RPCs have distinct request IDs and exact mission module identity', async () => {
  state().addWaypointAt(48.6493, -123.3982); await sync()
  const promise = state().setCurrent(0)
  await waitUntil(() => Boolean(pending('mission_set_current')))
  h.resolve('mission_set_current'); await promise
  const requests = h.requests.filter((request) => ['mission_upload', 'mission_download', 'mission_set_current'].includes(request.command))
  const contexts = requests.map((request) => request.args.request.context ?? request.args.request)
  assert(contexts.every((context) => context.session.module_id === 'mission'))
  assert.equal(new Set(contexts.map((context) => context.request_id)).size, contexts.length)
})
await check('height compensation stays on wire items and preserves coordinate-less commands', async () => {
  const waypoint = (meters, command = 16) => ({ position: { latitude_deg: 48.5, longitude_deg: -123.4 }, altitude: { datum: 'AMSL_EGM96', meters }, command, params: [0, 0, 0, 0], autocontinue: true, locked: false })
  assert.equal(h.compileWaypoints([waypoint(100)], 'amsl', 0, 5)[0].z, 95)
  assert.equal(h.compileWaypoints([waypoint(100)], 'amsl', 0)[0].z, 100)
  assert.equal(h.compileWaypoints([waypoint(12, 178)], 'amsl', 0, 5)[0].z, 12)
  assert.equal(h.compileWaypoints([waypoint(130)], 'relative', 30, 5)[0].z, 95)
  assert.equal(h.shortHash(''), '811c9dc5')
})
await h.reset()
if (failures.length) { for (const failure of failures) console.error(failure); process.exit(1) }
console.log(`check:mission-sync OK — ${passed} owned-store scenarios`)
