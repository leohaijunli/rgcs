// Every rejected `actuator_*`/log IPC call must render the native reason, not
// "[object Object]". Transpiles the real formatter with no browser or hardware.
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'

const source = await readFile(new URL('../src/actuator/errors.ts', import.meta.url), 'utf8')
const compiled = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
}).outputText
const { describeOperationFailure } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`)

function check(name, run) {
  run()
  console.log(`PASS: ${name}`)
}

function rejects(error) {
  const text = describeOperationFailure(error)
  assert.equal(typeof text, 'string')
  assert.notEqual(text, '[object Object]')
  assert.ok(text.length > 0)
  return text
}

check('motor protocol refusal from a START', () => {
  assert.equal(rejects({ code: 'protocol', error: { code: 'disabled' } }), 'motor control is OFF')
  assert.equal(rejects({ code: 'protocol', error: { code: 'service_stopped' } }), 'the motor service stopped')
  assert.equal(rejects({ code: 'protocol', error: { code: 'timed_out' } }), 'the motor operation deadline expired')
  assert.equal(
    rejects({ code: 'protocol', error: { code: 'permission_changed' } }),
    'the native motor permission/configuration changed',
  )
})

check('intermittent START refusals name the reason', () => {
  const busy = rejects({
    code: 'operation',
    error: { code: 'resource_busy', resource: 'actuator_output', holder: { id: { owner: 'bench', target: {} } } },
  })
  assert.match(busy, /still owns the actuator output/)
  assert.equal(rejects({ code: 'operation', error: { code: 'cancelled' } }), 'operation cancellation was requested')
  assert.equal(rejects({ code: 'operation', error: { code: 'deadline_exceeded' } }), 'the operation deadline expired')
  assert.equal(
    rejects({ code: 'operation', error: { code: 'runtime', error: { code: 'target_unavailable' } } }),
    'the requested target is unavailable or unverified',
  )
  assert.equal(
    rejects({ code: 'operation', error: { code: 'runtime', error: { code: 'target_changed' } } }),
    'the target session, route or transport generation changed',
  )
})

check('nested details keep their message', () => {
  assert.equal(
    rejects({ code: 'operation', error: { code: 'invalid_request', message: 'value count changed' } }),
    'value count changed',
  )
  assert.equal(
    rejects({ code: 'reconciliation_required', operation: { id: {} }, reason: 'late release ack' }),
    'the operation requires reconciliation: late release ack',
  )
  assert.equal(
    rejects({ code: 'operation', error: { code: 'runtime', error: { code: 'service_unavailable', service: 'run' } } }),
    'the run service is unavailable',
  )
})

check('capture and log refusals use the same formatter', () => {
  assert.equal(rejects({ code: 'capture', error: { kind: 'busy' } }), 'another capture exchange is active')
  assert.equal(
    rejects({ code: 'capture', error: { kind: 'transport', message: 'link lost' } }),
    'transport: link lost',
  )
  // `busy` is shared by the capture and log protocol maps; the formatter
  // resolves it to the capture wording. It stays a readable refusal either way.
  assert.equal(rejects({ code: 'protocol', error: { code: 'busy' } }), 'another capture exchange is active')
  assert.equal(rejects({ code: 'local_io', message: 'disk full' }), 'local io: disk full')
})

check('plain rejections pass through unchanged', () => {
  assert.equal(rejects('no serial PX4 target — connect it first'), 'no serial PX4 target — connect it first')
  assert.equal(rejects(new Error('motor start/tone cancelled')), 'motor start/tone cancelled')
  assert.equal(rejects(42), '42')
})

check('an unknown code is still readable', () => {
  assert.equal(rejects({ code: 'new_failure_mode' }), 'new failure mode')
  assert.equal(rejects({ some: 'object' }), '{"some":"object"}')
})

console.log('PASS motor errors: motor, operation, capture and log failures render native reasons (no [object Object])')
