#!/usr/bin/env node
// i18n key check (DEVELOPMENT_PLAN Phase 0 addendum, task 0.6).
//
// Every key referenced by `t('...')` / `i18n.t('...')` in src must exist in the
// English resource, so the UI never renders a raw key name. Template calls such
// as t(`settings.tab.${tab}`) contribute a prefix, which must match at least one
// key. Exits non-zero and lists the offenders.
//
//   node scripts/check-i18n.mjs
import { mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'

const srcDir = join(dirname(fileURLToPath(import.meta.url)), '..', 'src')

async function loadEnglish() {
  const out = join(await mkdtemp(join(tmpdir(), 'maggcs-i18n-')), 'en.mjs')
  await build({
    entryPoints: [join(srcDir, 'i18n', 'en.ts')],
    outfile: out,
    bundle: true,
    format: 'esm',
    platform: 'neutral',
    logLevel: 'silent',
  })
  const mod = await import(`file://${out}`)
  await rm(dirname(out), { recursive: true, force: true })
  return mod.default
}

function flatten(obj, prefix = '', out = new Set()) {
  for (const [key, value] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${key}` : key
    if (value && typeof value === 'object') flatten(value, path, out)
    else out.add(path)
  }
  return out
}

async function walk(dir) {
  const files = []
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) files.push(...(await walk(full)))
    else if (/\.tsx?$/.test(entry.name)) files.push(full)
  }
  return files
}

const keys = flatten(await loadEnglish())

// i18next resolves plurals via `_one` / `_other` suffixes, so a literal call
// like t('link.error.dropped', { count }) is satisfied by those variants.
const PLURAL_SUFFIXES = ['_zero', '_one', '_two', '_few', '_many', '_other']
function hasKey(key) {
  return keys.has(key) || PLURAL_SUFFIXES.some((s) => keys.has(`${key}${s}`))
}

const sources = (await walk(srcDir)).filter(
  (f) => !f.endsWith(join('i18n', 'en.ts')),
)

const missing = []
const prefixes = new Set()
for (const file of sources) {
  const text = await readFile(file, 'utf8')
  const rel = relative(srcDir, file)
  // Literal keys: t('a.b') / t("a.b") / i18n.t('a.b')
  for (const m of text.matchAll(/\bt\(\s*(['"])([\w.]+)\1/g)) {
    if (!hasKey(m[2])) missing.push(`${rel}: ${m[2]}`)
  }
  // Template prefixes: t(`a.b.${x}`)
  for (const m of text.matchAll(/\bt\(\s*`([\w.]+)\$\{/g)) {
    prefixes.add(m[1])
  }
}

const unknownPrefixes = [...prefixes].filter(
  (p) => ![...keys].some((k) => k.startsWith(p)),
)
for (const p of unknownPrefixes) missing.push(`template prefix: ${p}*`)

if (missing.length > 0) {
  console.error(`check:i18n FAILED — ${missing.length} missing key(s):`)
  for (const m of missing.sort()) console.error(`  ${m}`)
  process.exit(1)
}
console.log(
  `check:i18n OK — ${keys.size} keys, ${prefixes.size} dynamic prefix(es), 0 missing`,
)
