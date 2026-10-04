#!/usr/bin/env node
// WCAG contrast check for design tokens (ADR-010 / Phase 0 acceptance).
// Reads hex colors from index.css and asserts text/background pairs pass.
//   node check-contrast.mjs
import { readFile } from 'node:fs/promises'

const css = await readFile(new URL('../src/design-system/index.css', import.meta.url), 'utf8')

function extract(block) {
  const out = {}
  for (const m of block.matchAll(/--(mg-[a-z0-9-]+):\s*(#[0-9a-fA-F]{3,8}|rgba?\([^)]*\))/g)) {
    out[m[1]] = m[2]
  }
  return out
}

function parseHex(hex) {
  const h = hex.replace('#', '')
  const v = h.length === 3 ? h.split('').map((c) => c + c).join('') : h.padEnd(8, 'f')
  const r = parseInt(v.slice(0, 2), 16)
  const g = parseInt(v.slice(2, 4), 16)
  const b = parseInt(v.slice(4, 6), 16)
  return [r, g, b]
}

function lum(hex) {
  const [r, g, b] = parseHex(hex).map((c) => {
    const s = c / 255
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4)
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

function contrast(a, b) {
  const [l1, l2] = [lum(a), lum(b)].sort((x, y) => y - x)
  return (l1 + 0.05) / (l2 + 0.05)
}

const root = css.slice(css.indexOf(':root {'), css.indexOf('[data-theme="light"] {'))
const light = css.slice(css.indexOf('[data-theme="light"] {'), css.indexOf('/* Tailwind theme mapping */'))
const themes = { dark: extract(root), light: extract(light) }

const PAIRS = [
  ['ink', 'bg'],
  ['muted', 'bg'],
  ['accent', 'bg'],
  ['ink', 'panel'],
  ['muted', 'panel'],
  ['ok-ink', 'ok'],
]

let fail = 0
for (const [theme, t] of Object.entries(themes)) {
  console.log(`theme: ${theme}`)
  for (const [fg, bg] of PAIRS) {
    const a = t[`mg-${fg}`]
    const b = t[`mg-${bg}`]
    if (!a || !b) continue
    const r = contrast(a, b)
    const pass = r >= 4.5
    if (!pass) fail++
    console.log(`  ${fg} on ${bg}: ${r.toFixed(2)} ${pass ? 'OK' : 'FAIL (<4.5)'}`)
  }
}

if (fail > 0) {
  console.error(`contrast check: ${fail} pair(s) below 4.5:1`)
  process.exit(1)
}
console.log('contrast check: OK (all pairs >= 4.5:1)')