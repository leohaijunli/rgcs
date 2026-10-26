#!/usr/bin/env node
// Replay seek math (operator bug: dragging the inspector timeline would not land
// on the dragged position and no data rendered until the first drag). The seek
// must start `span` before the target so the forward-only ULog source's unpaced
// scrub fills exactly one visible window and ends the playhead at the target.
//
//   node scripts/check-replay-seek.mjs
//
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import assert from 'node:assert/strict'

const here = dirname(fileURLToPath(import.meta.url))
const srcDir = join(here, '..', 'src')

async function load(entry) {
  const out = join(await mkdtemp(join(tmpdir(), 'maggcs-seek-')), 'mod.mjs')
  await build({
    entryPoints: [join(srcDir, ...entry.split('/'))],
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

const { replaySeekWindow } = await load('inspector/replaySeek.ts')

const W30 = 30_000

// A seek in the middle of the log starts one window earlier and scrubs exactly
// that span, so the playhead lands on the dragged position.
assert.deepEqual(replaySeekWindow(60_000, W30), { sourceSeekMs: 30_000, scrubSpanMs: 30_000 })

// Near the start the source offset is clamped to 0; the scrub still spans one
// window so a full window of data is drawn.
assert.deepEqual(replaySeekWindow(10_000, W30), { sourceSeekMs: 0, scrubSpanMs: 30_000 })
assert.deepEqual(replaySeekWindow(0, W30), { sourceSeekMs: 0, scrubSpanMs: 30_000 })

// A custom window is honoured, not the backend's 30 s default.
assert.deepEqual(replaySeekWindow(120_000, 60_000), { sourceSeekMs: 60_000, scrubSpanMs: 60_000 })

console.log('check-replay-seek OK')