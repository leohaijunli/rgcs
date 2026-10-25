// Numerical checks of the real scope store, without a browser or hardware.
// TypeScript checking is performed separately by npm run typecheck.
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'

const source = await readFile(new URL('../src/actuator/scopeStore.ts', import.meta.url), 'utf8')
const compiled = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
}).outputText
const { ScopeStore, normalizeTraceColor } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`)

function output(connection_id, revision, unit = 'microseconds', motor = 1) {
  return { connection_id, revision, motor, channel: { port: motor === 1 ? 'main' : 'aux', channel: 1 }, unit }
}

function check(name, run) {
  run()
  console.log(`PASS: ${name}`)
}

check('stable definitions and canonical configuration identity', () => {
  const store = new ScopeStore()
  const first = output(17, 1)
  assert.deepEqual(store.configureOutputs([first]), { ok: true })
  const def = store.definitionOf('cmd.pwm1')
  assert.equal(def.unit, 'µs')
  assert.equal(store.definitionOf('cmd.pwm1'), def)
  store.ingest([{ id: 'cmd.pwm1', t: 1, v: 1500, output: first }])
  assert.deepEqual(store.configureOutputs([{
    unit: first.unit, channel: { channel: 1, port: 'main' },
    motor: 1, revision: 1, connection_id: 17,
  }]), { ok: true })
  assert.equal(store.definitionOf('cmd.pwm1'), def, 'object key order does not rebuild a definition')
  assert.equal(store.rings.get('cmd.pwm1').length, 1, 'unchanged configuration retains history')
})

check('PWM/DShot/pulse-unit changes isolate history and reject late samples', () => {
  const store = new ScopeStore()
  const pwm = output(17, 1)
  store.configureOutputs([pwm])
  store.ingest([
    { id: 'cmd.pwm1', t: 1, v: 1500, output: pwm },
    { id: 'fc.motor1', t: 1.1, v: 1490, output: pwm },
    { id: 'mag.raw_total', t: 1, v: 50000 },
  ])
  const dshot = output(17, 2, 'd_shot_throttle')
  assert.deepEqual(store.configureOutputs([dshot]), { ok: true })
  assert.equal(store.definitionOf('cmd.pwm1').unit, 'thr')
  assert.equal(store.definitionOf('fc.motor1').unit, 'thr')
  assert.equal(store.rings.has('cmd.pwm1'), false)
  assert.equal(store.rings.has('fc.motor1'), false)
  assert.equal(store.rings.get('mag.raw_total').length, 1)
  store.ingest([
    { id: 'cmd.pwm1', t: 2, v: 1500, output: pwm },
    { id: 'cmd.pwm1', t: 2.1, v: 650 },
    { id: 'cmd.pwm1', t: 2.2, v: 650, output: dshot },
    { id: 'fc.motor1', t: 2.3, v: 640, output: dshot },
    { id: 'cmd.pwm1', t: 2.4, v: 1700, output: { ...dshot, unit: 'microseconds' } },
  ])
  assert.equal(store.rings.get('cmd.pwm1').length, 1)
  assert.equal(store.stats.get('cmd.pwm1').last, 650)
  assert.deepEqual(store.configureOutputs([pwm]), { ok: false, reason: 'obsolete_configuration' })
  assert.deepEqual(store.configureOutputs([{ ...dshot, unit: 'microseconds' }]), { ok: false, reason: 'invalid_configuration' })
  assert.equal(store.stats.get('cmd.pwm1').last, 650)
  const pulse = output(17, 3)
  assert.deepEqual(store.configureOutputs([pulse]), { ok: true })
  assert.equal(store.definitionOf('cmd.pwm1').unit, 'µs')
  assert.equal(store.rings.has('cmd.pwm1'), false)
  store.ingest([
    { id: 'cmd.pwm1', t: 3, v: 750, output: dshot },
    { id: 'cmd.pwm1', t: 3.1, v: 1400, output: pulse },
  ])
  assert.equal(store.rings.get('cmd.pwm1').length, 1)
  assert.equal(store.stats.get('cmd.pwm1').last, 1400)
})

check('mapped MAIN/AUX feedback cannot merge or borrow another motor metadata', () => {
  const store = new ScopeStore()
  const main = output(17, 1)
  const aux = output(17, 1, 'd_shot_throttle', 2)
  store.configureOutputs([main, aux])
  store.ingest([
    { id: 'fc.motor1', t: 1, v: 1500, output: main },
    { id: 'fc.motor2', t: 1, v: 750, output: aux },
    { id: 'fc.motor1', t: 2, v: 750, output: aux },
    { id: 'fc.servo1', t: 2, v: 1500, output: main },
  ])
  assert.equal(store.stats.get('fc.motor1').last, 1500)
  assert.equal(store.stats.get('fc.motor2').last, 750)
  assert.equal(store.rings.get('fc.motor1').length, 1)
  assert.equal(store.rings.has('fc.servo1'), false, 'unmapped legacy feedback is not a motor trace')
  assert.match(store.definitionOf('fc.motor1').label, /MAIN1/)
  assert.match(store.definitionOf('fc.motor2').label, /AUX1/)
  assert.equal(store.definitionOf('fc.motor1').unit, 'µs')
  assert.equal(store.definitionOf('fc.motor2').unit, 'thr')
  const remapped = { ...main, revision: 2, channel: { port: 'main', channel: 5 } }
  store.configureOutputs([remapped, { ...aux, revision: 2 }])
  assert.equal(store.rings.has('fc.motor1'), false)
  assert.match(store.definitionOf('fc.motor1').label, /MAIN5/)
})

check('new connections retire every older connection even with a larger revision', () => {
  const store = new ScopeStore()
  const old = output(17, 100)
  store.configureOutputs([old])
  store.ingest([{ id: 'cmd.pwm1', t: 1, v: 1500, output: old }])
  const current = output(18, 1)
  assert.deepEqual(store.configureOutputs([current]), { ok: true })
  assert.equal(store.rings.has('cmd.pwm1'), false)
  const late = { ...old, revision: 999 }
  assert.deepEqual(store.configureOutputs([late]), { ok: false, reason: 'obsolete_configuration' })
  store.ingest([{ id: 'cmd.pwm1', t: 2, v: 1600, output: late }])
  assert.equal(store.rings.has('cmd.pwm1'), false)
})

check('invalid configuration and unavailable units never become guessed output data', () => {
  const store = new ScopeStore()
  const first = output(17, 1)
  for (const config of [
    [{ ...first, unit: 'unknown' }],
    [{ ...first, channel: { port: 'main', channel: 0 } }],
    [{ ...first, revision: 0 }],
    [{ ...first, connection_id: Number.MAX_SAFE_INTEGER + 1 }],
    [first, { ...first, motor: 2 }],
    [{ ...first, motor: 2 }],
  ]) assert.deepEqual(store.configureOutputs(config), { ok: false, reason: 'invalid_configuration' })
  assert.equal(store.definitionOf('cmd.pwm1').unit, 'raw')
  store.configureOutputs([first])
  store.ingest([{ id: 'cmd.pwm1', t: 1, v: 1500, output: first }])
  store.configureOutputs([])
  store.ingest([{ id: 'cmd.pwm1', t: 2, v: 1500, output: first }])
  assert.equal(store.rings.has('cmd.pwm1'), false)
  assert.equal(store.definitionOf('cmd.pwm1').unit, 'raw')
})

check('clear and reconfiguration retain selection memory; stores have separate definitions', () => {
  const first = new ScopeStore()
  const second = new ScopeStore()
  const pwm = output(17, 1)
  const dshot = output(18, 1, 'd_shot_throttle')
  first.preloadOffered(['cmd.pwm1'])
  first.configureOutputs([pwm])
  second.configureOutputs([dshot])
  assert.equal(first.definitionOf('cmd.pwm1').unit, 'µs')
  assert.equal(second.definitionOf('cmd.pwm1').unit, 'thr')
  first.ingest([{ id: 'cmd.pwm1', t: 1, v: 1500, output: pwm }])
  first.clear()
  first.ingest([{ id: 'cmd.pwm1', t: 2, v: 1400, output: pwm }])
  assert.equal(first.stats.get('cmd.pwm1').last, 1400)
  assert.deepEqual(first.takeNew(), [])
})

check('explicit three-sample release gaps survive and malformed gaps are ignored', () => {
  const store = new ScopeStore()
  const pwm = output(17, 1)
  store.configureOutputs([pwm])
  store.ingest([
    { id: 'cmd.pwm1', t: 1, v: null, gap: 'released', output: pwm },
    { id: 'cmd.pwm1', t: 1, v: 1500, output: pwm },
    { id: 'cmd.pwm1', t: 2, v: null, output: pwm },
    { id: 'cmd.pwm1', t: 2, v: null, gap: 'released', output: pwm },
    { id: 'cmd.pwm1', t: 3, v: 1600, output: pwm },
  ])
  const ring = store.rings.get('cmd.pwm1')
  assert.equal(ring.length, 3)
  assert.equal(ring.valueAt(0), 1500)
  assert.equal(Number.isNaN(ring.valueAt(1)), true)
  assert.equal(ring.valueAt(2), 1600)
  assert.equal(store.stateOf('cmd.pwm1', 3), 'live')
})

check('stale gaps are inserted once at each signal deadline and recovery does not bridge', () => {
  const store = new ScopeStore()
  store.ingest([{ id: 'esc1.rpm', t: 1, v: 5000 }])
  store.advanceTime(1.49)
  assert.equal(store.rings.get('esc1.rpm').length, 1)
  store.advanceTime(1.51)
  const ring = store.rings.get('esc1.rpm')
  assert.equal(ring.length, 2)
  assert.equal(ring.timeAt(1), 1.5)
  assert.equal(Number.isNaN(ring.valueAt(1)), true)
  assert.equal(store.stateOf('esc1.rpm', 1.51), 'stale')
  store.advanceTime(9)
  assert.equal(ring.length, 2, 'one stale interval gets one boundary')
  store.ingest([{ id: 'esc1.rpm', t: 10, v: 5100 }])
  assert.equal(ring.length, 3)
  assert.equal(ring.valueAt(2), 5100)
  assert.equal(store.stateOf('esc1.rpm', 10), 'live')
  store.advanceTime(10.51)
  assert.equal(ring.length, 4, 'a later stale interval gets its own boundary')
})

check('run end breaks existing command, mapped feedback and RPM but not absent or unrelated signals', () => {
  const store = new ScopeStore()
  const pwm = output(17, 1)
  store.configureOutputs([pwm])
  store.ingest([
    { id: 'session.running', t: 1, v: 1 },
    { id: 'cmd.pwm1', t: 1.1, v: 1500, output: pwm },
    { id: 'fc.motor1', t: 1.1, v: 1498, output: pwm },
    { id: 'esc1.rpm', t: 1.1, v: 5000 },
    { id: 'esc1.current', t: 1.1, v: 4 },
    { id: 'session.running', t: 2, v: 0 },
  ])
  for (const id of ['cmd.pwm1', 'fc.motor1', 'esc1.rpm']) {
    const ring = store.rings.get(id)
    assert.equal(ring.length, 2, id)
    assert.equal(ring.timeAt(1), 2, id)
    assert.equal(Number.isNaN(ring.valueAt(1)), true, id)
    assert.equal(store.stateOf(id, 2), 'boundary', id)
  }
  assert.equal(store.rings.get('esc1.current').length, 1)
  assert.equal(store.rings.has('cmd.pwm2'), false)
  store.ingest([
    { id: 'session.running', t: 3, v: 1 },
    { id: 'cmd.pwm1', t: 3.1, v: 1600, output: pwm },
    { id: 'fc.motor1', t: 3.1, v: 1598, output: pwm },
    { id: 'esc1.rpm', t: 3.1, v: 5200 },
  ])
  for (const id of ['cmd.pwm1', 'fc.motor1', 'esc1.rpm']) {
    const ring = store.rings.get(id)
    assert.equal(ring.length, 3, id)
    assert.equal(Number.isNaN(ring.valueAt(1)), true, id)
    assert.equal(store.stateOf(id, 3.1), 'live', id)
  }
  assert.deepEqual(store.runs, [{ t0: 1, t1: 2 }, { t0: 3, t1: null }])
})

check('elapsed time starts at the first plotted value and Clear resets it', () => {
  const store = new ScopeStore()
  const pwm = output(17, 1)
  store.configureOutputs([pwm])
  store.ingest([
    { id: 'session.running', t: 90, v: 1 },
    { id: 'cmd.pwm1', t: 95, v: null, gap: 'released', output: pwm },
  ])
  assert.equal(store.originT, null, 'markers and gaps do not start the clock')
  store.ingest([{ id: 'cmd.pwm1', t: 100, v: 1500, output: pwm }])
  assert.equal(store.originT, 100)
  assert.equal(store.elapsedAt(100), 0)
  assert.equal(store.elapsedAt(205.25), 105.25)
  store.configureOutputs([{ ...pwm, revision: 2, unit: 'd_shot_throttle' }])
  assert.equal(store.elapsedAt(210), 110, 'configuration changes preserve elapsed time')
  store.clear()
  assert.equal(store.originT, null)
  assert.equal(store.elapsedAt(500), 0)
  store.ingest([{ id: 'mag.raw_total', t: 500, v: 50_000 }])
  assert.equal(store.elapsedAt(503), 3)
})

check('trace colors accept only canonical six-digit hex values', () => {
  assert.equal(normalizeTraceColor('#4EA1FF'), '#4ea1ff')
  for (const value of ['4ea1ff', '#fff', '#12345678', '#zzzzzz', '', null, 123]) {
    assert.equal(normalizeTraceColor(value), null)
  }
})
