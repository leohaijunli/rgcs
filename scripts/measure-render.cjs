#!/usr/bin/env node
// Reports how much non-background content a PNG has, for scripts/desktop-smoke.sh.
//
// Usage: NODE_PATH=frontend/node_modules node scripts/measure-render.cjs shot.png
// Prints JSON: { "colours": n, "brightPct": x.y, "maxLum": n }

'use strict';

const fs = require('fs');

let PNG;
try {
  ({ PNG } = require('pngjs'));
} catch (error) {
  console.error('measure-render: pngjs not found; set NODE_PATH to frontend/node_modules');
  process.exit(2);
}

const file = process.argv[2];
if (!file) {
  console.error('usage: measure-render.cjs <png>');
  process.exit(2);
}

const png = PNG.sync.read(fs.readFileSync(file));
const colours = new Set();
let bright = 0;
let maxLum = 0;
const pixels = png.width * png.height;

for (let i = 0; i < pixels; i += 1) {
  const offset = i << 2;
  const r = png.data[offset];
  const g = png.data[offset + 1];
  const b = png.data[offset + 2];
  const lum = (r + g + b) / 3;
  if (lum > maxLum) maxLum = lum;
  if (lum > 120) bright += 1;
  colours.add(`${r >> 4},${g >> 4},${b >> 4}`);
}

console.log(
  JSON.stringify({
    colours: colours.size,
    brightPct: Number(((100 * bright) / pixels).toFixed(2)),
    maxLum: Math.round(maxLum),
  })
);
