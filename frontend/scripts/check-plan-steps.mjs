#!/usr/bin/env node
// Plan readiness checks (U17): the status dot on each planning step card and
// the summary's "needs attention" state both come from `mission/readiness.ts`.
// Drives that pure module and asserts what the panel shows at each point in
// the workflow — so a wrong dot, or a blocked step that reads as done, is
// caught here rather than on the flight line.
//
//   node scripts/check-plan-steps.mjs
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')

async function bundle(entry) {
  const out = join(await mkdtemp(join(tmpdir(), 'maggcs-plan-steps-')), 'mod.mjs')
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

const { planSteps, planBlockers } = await bundle('mission/readiness.ts')

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
function eq(a, b, msg) {
  assert(a === b, `${msg ?? 'not equal'}: ${JSON.stringify(a)} !== ${JSON.stringify(b)}`)
}

/** Status lookup by id, so a case reads as the card it describes. */
function statusOf(facts, id) {
  const step = planSteps(facts).find((s) => s.id === id)
  assert(step, `no step ${id}`)
  return step.status
}

const base = {
  hasArea: false,
  hasPattern: false,
  itemCount: 0,
  sync: 'empty',
  drapeError: null,
}

check('the list always covers the four workflow steps in order', () => {
  const ids = planSteps(base).map((s) => s.id)
  assert(
    ids.join(',') === 'area,pattern,waypoints,sync',
    `order/ids wrong: ${ids.join(',')}`,
  )
})

check('a fresh plan has every step still to do', () => {
  const f = { ...base }
  for (const id of ['area', 'pattern', 'waypoints', 'sync']) {
    eq(statusOf(f, id), 'todo', id)
  }
})

check('a defined boundary completes only the Area step', () => {
  const f = { ...base, hasArea: true }
  eq(statusOf(f, 'area'), 'done', 'area')
  eq(statusOf(f, 'pattern'), 'todo', 'pattern still todo')
  eq(statusOf(f, 'waypoints'), 'todo', 'waypoints still todo')
})

check('a generated trajectory completes Area and Pattern', () => {
  const f = { ...base, hasArea: true, hasPattern: true }
  eq(statusOf(f, 'pattern'), 'done', 'pattern')
  // No items yet: the trajectory readout exists but nothing is flyable.
  eq(statusOf(f, 'waypoints'), 'todo', 'waypoints')
})

check('generated, unsynced waypoints complete Waypoints, leave Sync to do', () => {
  const f = { ...base, hasArea: true, hasPattern: true, itemCount: 42, sync: 'unsynced' }
  eq(statusOf(f, 'waypoints'), 'done', 'waypoints')
  eq(statusOf(f, 'sync'), 'todo', 'sync')
})

check('an in-sync plan completes the Sync step', () => {
  const f = { ...base, hasArea: true, hasPattern: true, itemCount: 42, sync: 'synced' }
  eq(statusOf(f, 'sync'), 'done', 'sync')
})

check('an FC mismatch blocks the Sync step (not just todo)', () => {
  const f = { ...base, hasArea: true, hasPattern: true, itemCount: 42, sync: 'mismatch' }
  eq(statusOf(f, 'sync'), 'blocked', 'sync')
})

check('a failed terrain drape blocks the Pattern step (where the drape is fixed)', () => {
  const f = {
    ...base,
    hasArea: true,
    hasPattern: true,
    itemCount: 42,
    sync: 'unsynced',
    drapeError: 'vertical datum chain unverified',
  }
  eq(statusOf(f, 'pattern'), 'blocked', 'pattern')
  eq(statusOf(f, 'waypoints'), 'done', 'waypoints still holds the items')
})

check('local edits after a sync read as dirty, not blocked', () => {
  const f = { ...base, hasArea: true, hasPattern: true, itemCount: 42, sync: 'dirty' }
  eq(statusOf(f, 'sync'), 'todo', 'sync')
})

check('an empty plan never reports Sync as blocked', () => {
  const f = { ...base, drapeError: null }
  eq(statusOf(f, 'sync'), 'todo', 'sync')
})

check('a clean plan has no blockers', () => {
  const f = { ...base, hasArea: true, hasPattern: true, itemCount: 42, sync: 'synced' }
  eq(planBlockers(f).length, 0, 'blockers')
})

check('a failed drape yields one blocker on the Pattern/drape field', () => {
  const f = { ...base, hasArea: true, hasPattern: true, itemCount: 42, drapeError: 'boom' }
  const bs = planBlockers(f)
  eq(bs.length, 1, 'count')
  eq(bs[0].step, 'pattern', 'step')
  eq(bs[0].field, 'drape', 'field')
  eq(bs[0].code, 'drape_failed', 'code')
  eq(bs[0].detail, 'boom', 'detail carried through')
})

check('an FC mismatch yields one blocker on the Sync field', () => {
  const f = { ...base, hasArea: true, hasPattern: true, itemCount: 42, sync: 'mismatch' }
  const bs = planBlockers(f)
  eq(bs.length, 1, 'count')
  eq(bs[0].step, 'sync', 'step')
  eq(bs[0].field, 'sync', 'field')
  eq(bs[0].code, 'fc_mismatch', 'code')
})

check('a mismatch with no local items is not a blocker', () => {
  const f = { ...base, sync: 'mismatch' }
  eq(planBlockers(f).length, 0, 'blockers')
})

check('the blocked dot and the blocker chip always agree on the step', () => {
  const f = {
    ...base,
    hasArea: true,
    hasPattern: true,
    itemCount: 42,
    sync: 'mismatch',
    drapeError: 'boom',
  }
  const blocked = new Set(planSteps(f).filter((s) => s.status === 'blocked').map((s) => s.id))
  const chipSteps = new Set(planBlockers(f).map((b) => b.step))
  eq([...blocked].sort().join(','), [...chipSteps].sort().join(','), 'blocked steps')
})

if (failures.length > 0) {
  console.error(`\ncheck:plan-steps FAILED — ${failures.length} failure(s):`)
  for (const f of failures) console.error(`  ${f}`)
  process.exit(1)
}
console.log(`\ncheck:plan-steps OK — ${passed} scenario(s)`)
