// Bundle the real store and exercise it against deterministic typed native RPCs.
import { build } from 'esbuild'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { webcrypto } from 'node:crypto'

const src = join(dirname(fileURLToPath(import.meta.url)), '../src')
const clone = (value) => structuredClone(value)
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b)
export async function tick() { await new Promise((resolve) => setTimeout(resolve, 0)) }
export async function waitUntil(predicate) {
  for (let index = 0; index < 1000; index += 1) {
    if (predicate()) return
    await new Promise((resolve) => setTimeout(resolve, 1))
  }
  throw new Error('Mission fixture did not reach the expected state')
}

export async function missionFixture() {
  if (!globalThis.crypto) globalThis.crypto = webcrypto
  const storage = new Map()
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: (key) => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, String(value)),
    removeItem: (key) => storage.delete(key), clear: () => storage.clear(),
  } })
  const fixture = { requests: [], pending: [], sessions: new Map(), fcItems: [], revision: 1, available: true,
    operationCounter: 0, sessionCounter: 0, cancelCalls: [], deferCancel: false, deferTargets: false, targetsWaiters: [] }
  fixture.target = () => ({ vehicle_id: 'mission-vehicle', session_id: `session-${fixture.revision}`,
    link_id: '1', generation: String(fixture.revision), mavlink_target: { system_id: 42, component_id: 1 } })
  const status = (entry, phase = 'running') => ({ operation: clone(entry.operation), request_id: entry.context.request_id,
    resources: ['mission_transfer'], phase, cleanup: phase === 'cancel_requested' ? 'pending' : 'not_required',
    outcome: null, stop_reason: phase === 'cancel_requested' ? 'owner_request' : null, remaining_budget_ms: 30000 })
  fixture.invoke = async (command, args = {}) => {
    fixture.requests.push({ command, args: clone(args) })
    if (command === 'runtime_targets') {
      if (fixture.deferTargets) await new Promise((resolve) => fixture.targetsWaiters.push(resolve))
      return [{ target: fixture.available ? fixture.target() : null, link_id: '1', availability: fixture.available ? 'available' : 'disconnected',
        roles: ['primary'], transport_owner: 'primary', endpoint: 'fixture', parameter_service_available: true, heartbeat_age_ms: 0 }]
    }
    if (command === 'runtime_client_attach') {
      if (!fixture.available || !same(args.target, fixture.target())) throw { code: 'runtime', error: { code: 'target_changed' } }
      const attached = { owner: { client_id: 'fixture-client', instance_id: `module-${++fixture.sessionCounter}` }, target: clone(args.target), module_id: args.moduleId }
      fixture.sessions.set(args.moduleId, attached)
      return clone(attached)
    }
    if (command === 'runtime_client_validate') {
      if (!same(fixture.sessions.get(args.session.module_id), args.session)) throw { code: 'not_owner' }
      if (!fixture.available || !same(args.session.target, fixture.target())) throw { code: 'runtime', error: { code: 'target_changed' } }
      return null
    }
    if (command === 'runtime_client_detach') {
      if (same(fixture.sessions.get(args.session.module_id), args.session)) fixture.sessions.delete(args.session.module_id)
      return null
    }
    if (command === 'mission_cancel') {
      fixture.cancelCalls.push(clone(args))
      const entry = fixture.pending.find((entry) => same(entry.operation, args.operation) && same(entry.context.session, args.session))
      if (!entry) throw { code: 'not_owner' }
      if (!fixture.deferCancel) queueMicrotask(() => fixture.reject(entry.command, {
        code: 'reconciliation_required', operation: clone(entry.operation), reason: 'Local retries retired; FC mission requires read-back',
      }))
      return status(entry, 'cancel_requested')
    }
    if (['mission_upload', 'mission_download', 'mission_clear', 'mission_set_current'].includes(command)) {
      const context = args.request.context ?? args.request
      if (!same(context.session.target, fixture.target())) throw { code: 'runtime', error: { code: 'target_changed' } }
      return new Promise((resolve, reject) => {
        const entry = { command, input: clone(args.request), context: clone(context), resolve, reject,
          operation: { id: `operation-${++fixture.operationCounter}`, owner: clone(context.session.owner), target: clone(context.session.target) } }
        fixture.pending.push(entry)
        queueMicrotask(() => fixture.store.getState().handleOperation(status(entry)))
      })
    }
    return null
  }
  globalThis.__missionFixtureInvoke = fixture.invoke
  const directory = await mkdtemp(join(tmpdir(), 'maggcs-owned-mission-'))
  const file = join(directory, 'fixture.mjs')
  await build({ stdin: { contents: `export {useMissionStore} from './stores/mission'; export {itemsHash,shortHash} from './mission/hash';
    export {planSyncStatus,planSyncPhase} from './mission/sync'; export {compileWaypoints,waypointFromItem} from './mission/compile';`, resolveDir: src, loader: 'ts' },
    outfile: file, bundle: true, format: 'esm', platform: 'neutral', logLevel: 'silent',
    plugins: [{ name: 'native-fixture', setup(build) {
      build.onResolve({ filter: /^@tauri-apps\// }, (args) => ({ path: args.path, namespace: 'native-fixture' }))
      build.onLoad({ filter: /.*/, namespace: 'native-fixture' }, () => ({ loader: 'js', contents:
        'export const invoke=(command,args)=>globalThis.__missionFixtureInvoke(command,args);' +
        'export const readTextFile=async()=>"";export const writeTextFile=async()=>{};export const open=async()=>null;export const save=async()=>null;export const listen=async()=>()=>{};' }))
    } }],
  })
  const module = await import(`file://${file}`)
  await rm(directory, { recursive: true, force: true })
  fixture.store = module.useMissionStore
  Object.assign(fixture, module)
  fixture.resolve = (command, result = null, operation = null) => {
    const index = fixture.pending.findIndex((entry) => entry.command === command)
    if (index < 0) throw new Error(`No pending ${command}`)
    const [entry] = fixture.pending.splice(index, 1)
    if (result === null) {
      if (command === 'mission_upload') {
        fixture.fcItems = clone(entry.input.items)
        result = { kind: 'uploaded', count: entry.input.items.length }
      } else if (command === 'mission_download') result = { kind: 'downloaded', items: clone(fixture.fcItems) }
      else if (command === 'mission_clear') { fixture.fcItems = []; result = { kind: 'cleared' } }
      else result = { kind: 'current', seq: entry.input.seq }
    }
    entry.resolve({ operation: operation ?? clone(entry.operation), result })
    return entry
  }
  fixture.reject = (command, error) => {
    const index = fixture.pending.findIndex((entry) => entry.command === command)
    if (index < 0) throw new Error(`No pending ${command}`)
    const [entry] = fixture.pending.splice(index, 1)
    entry.reject(error)
    return entry
  }
  fixture.progress = (entry, sent, total, overrides = {}) => fixture.store.getState().handleProgress({
    operation: clone(entry.operation), request_id: entry.context.request_id, sent, total, ...overrides,
  })
  fixture.current = (seq, target = fixture.target(), at = Date.now()) => fixture.store.getState().handleCurrent({ target: clone(target), seq, received_at_ms: at })
  fixture.reset = async () => {
    fixture.store.getState().reset()
    for (const entry of fixture.pending.splice(0)) entry.reject({ code: 'cancelled' })
    for (const resolve of fixture.targetsWaiters.splice(0)) resolve()
    fixture.deferTargets = false
    await tick(); await tick()
    fixture.requests = []; fixture.cancelCalls = []; fixture.fcItems = []; fixture.available = true; fixture.deferCancel = false
  }
  return fixture
}
