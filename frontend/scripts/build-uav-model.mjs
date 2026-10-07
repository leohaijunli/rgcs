// Build a static (non-skinned) copy of the UAV model for Cesium.
//
// Cesium's glTF loader builds draw commands for the skinned primitives in
// scene.gltf but does not rasterize them, so the imported drone is invisible.
// This script strips skins/animations plus the attributes only they need
// (JOINTS_0, WEIGHTS_0) and repacks the attributes the materials actually use
// into a tight, unstrided buffer. The result is a smaller glTF that Cesium
// renders reliably.
//
// Usage: node scripts/build-uav-model.mjs
//
// Source asset: "animated drone with camera (FREE)" by ulunkwulunk, CC-BY-4.0
// (see model/LICENSE.txt and public/model/README.md at the repo root).

import { copyFileSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const sourceDir = join(here, '..', '..', 'model')
const outDir = join(here, '..', 'public', 'model')
const SRC_GLTF = join(sourceDir, 'scene.gltf')
const OUT_GLTF = join(outDir, 'scene-static.gltf')
const OUT_BIN = join(outDir, 'scene-static.bin')

// Mesh attributes kept in the static model. Everything else (JOINTS_0,
// WEIGHTS_0, TANGENT, TEXCOORD_1..4) is unused by the materials.
const KEEP_ATTRIBUTES = new Set(['POSITION', 'NORMAL', 'TEXCOORD_0'])

const COMPONENT_BYTES = { 5120: 1, 5121: 1, 5122: 2, 5123: 2, 5125: 4, 5126: 4 }
const TYPE_COMPONENTS = { SCALAR: 1, VEC2: 2, VEC3: 3, VEC4: 4, MAT2: 4, MAT3: 9, MAT4: 16 }

function elementBytes(accessor) {
  const components = TYPE_COMPONENTS[accessor.type]
  const componentBytes = COMPONENT_BYTES[accessor.componentType]
  if (!components || !componentBytes) {
    throw new Error(`unsupported accessor ${accessor.type}/${accessor.componentType}`)
  }
  return components * componentBytes
}

const gltf = JSON.parse(readFileSync(SRC_GLTF, 'utf8'))
if (gltf.buffers.length !== 1) {
  throw new Error(`expected a single buffer, found ${gltf.buffers.length}`)
}
const sourceBin = readFileSync(join(sourceDir, gltf.buffers[0].uri))

// 1. Strip skinning and animation so no node/primitive requests a skin.
delete gltf.skins
delete gltf.animations
for (const node of gltf.nodes) delete node.skin

// 2. Drop unused attributes and collect the accessors that remain reachable.
const usedAccessors = new Set()
for (const mesh of gltf.meshes) {
  for (const primitive of mesh.primitives) {
    for (const name of Object.keys(primitive.attributes)) {
      if (!KEEP_ATTRIBUTES.has(name)) {
        delete primitive.attributes[name]
        continue
      }
      usedAccessors.add(primitive.attributes[name])
    }
    if (primitive.indices !== undefined) usedAccessors.add(primitive.indices)
  }
}

// 3. Repack each used accessor into its own tight, unstrided bufferView,
//    de-interleaving where the source bufferView carries a byteStride.
const chunks = []
let byteOffset = 0
function append(buffer) {
  const pad = (4 - (byteOffset % 4)) % 4
  if (pad > 0) {
    chunks.push(Buffer.alloc(pad))
    byteOffset += pad
  }
  const start = byteOffset
  chunks.push(buffer)
  byteOffset += buffer.length
  return start
}

const remap = new Map()
const accessors = []
const bufferViews = []
for (const index of [...usedAccessors].sort((a, b) => a - b)) {
  const accessor = gltf.accessors[index]
  if (accessor.sparse) {
    throw new Error(`accessor ${index} is sparse, which is not supported`)
  }
  const view = gltf.bufferViews[accessor.bufferView]
  const size = elementBytes(accessor)
  const stride = view.byteStride && view.byteStride > 0 ? view.byteStride : size
  const base = (view.byteOffset || 0) + (accessor.byteOffset || 0)
  const packed = Buffer.alloc(accessor.count * size)
  for (let i = 0; i < accessor.count; i++) {
    sourceBin.copy(packed, i * size, base + i * stride, base + i * stride + size)
  }
  const start = append(packed)
  bufferViews.push({ buffer: 0, byteOffset: start, byteLength: packed.length })
  const result = {
    componentType: accessor.componentType,
    count: accessor.count,
    type: accessor.type,
    bufferView: bufferViews.length - 1,
  }
  if (accessor.normalized) result.normalized = true
  if (accessor.min) result.min = accessor.min
  if (accessor.max) result.max = accessor.max
  remap.set(index, accessors.length)
  accessors.push(result)
}

// 4. Point every primitive at the repacked accessors.
for (const mesh of gltf.meshes) {
  for (const primitive of mesh.primitives) {
    const attributes = {}
    for (const [name, ref] of Object.entries(primitive.attributes)) {
      attributes[name] = remap.get(ref)
    }
    primitive.attributes = attributes
    if (primitive.indices !== undefined) primitive.indices = remap.get(primitive.indices)
  }
}

gltf.accessors = accessors
gltf.bufferViews = bufferViews
const packedBin = Buffer.concat(chunks)
gltf.buffers = [{ byteLength: packedBin.length, uri: 'scene-static.bin' }]

mkdirSync(outDir, { recursive: true })
for (const image of gltf.images || []) {
  if (!image.uri) continue
  const target = join(outDir, image.uri)
  mkdirSync(dirname(target), { recursive: true })
  copyFileSync(join(sourceDir, image.uri), target)
}
writeFileSync(OUT_BIN, packedBin)
writeFileSync(OUT_GLTF, JSON.stringify(gltf))

const mb = (path) => (statSync(path).size / 1024 / 1024).toFixed(2)
console.log(`accessors: ${gltf.accessors.length}, bufferViews: ${gltf.bufferViews.length}`)
console.log(`${OUT_BIN}: ${mb(OUT_BIN)} MB`)
console.log(`${OUT_GLTF}: ${mb(OUT_GLTF)} MB`)
