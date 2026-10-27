// ADR-002 / AF-02b.4b: compare the complete core + native ts-rs output to the
// frontend mirror. Regeneration is performed by Rust's export_bindings tests.
import { readdir, readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const mirrorDir = join(root, 'frontend/src/generated-types')
const sourceDirs = ['crates/core/bindings', 'crates/app-tauri/bindings']
const canonical = new Map()
const failures = []

for (const sourceDir of sourceDirs) {
  const files = (await readdir(join(root, sourceDir))).filter((name) => name.endsWith('.ts')).sort()
  for (const name of files) {
    const content = await readFile(join(root, sourceDir, name), 'utf8')
    if (canonical.has(name) && canonical.get(name).content !== content) {
      failures.push(`conflicting Rust exports: ${name}`)
    }
    canonical.set(name, { sourceDir, content })
  }
}

const mirrorFiles = (await readdir(mirrorDir)).filter((name) => name.endsWith('.ts')).sort()
for (const [name, { sourceDir, content }] of canonical) {
  if (!mirrorFiles.includes(name)) {
    failures.push(`missing frontend mirror: ${sourceDir}/${name}`)
  } else if (await readFile(join(mirrorDir, name), 'utf8') !== content) {
    failures.push(`stale frontend mirror: ${sourceDir}/${name}`)
  }
}
for (const name of mirrorFiles) {
  if (!canonical.has(name)) failures.push(`frontend type has no canonical Rust export: ${name}`)
}

if (failures.length > 0) {
  for (const failure of failures) console.error(`FAIL ${failure}`)
  console.error('Regenerate export_bindings for core and app, including TS_RS_EXPORT_DIR for the frontend mirror.')
  process.exitCode = 1
} else {
  console.log(`PASS ${canonical.size} Rust-derived TypeScript contracts match the complete frontend mirror`)
}
