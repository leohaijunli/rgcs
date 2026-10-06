// Runtime-generated glTF for the 3D UAV marker, so the repository ships no
// binary model asset and the shape stays editable in code.
//
// Author axes: +X = nose, +Y = up, +Z = right wing. Cesium applies its glTF
// axis correction (y-up/z-forward -> east-north-up) after the entity transform
// and lands glTF +X on local north, +Y on up and +Z on east, so a heading of 0
// points the nose north.

type Vec3 = readonly [number, number, number]

interface Part {
  center: Vec3
  size: Vec3
}

const PARTS: readonly Part[] = [
  { center: [0, 0, 0], size: [4, 0.5, 0.5] }, // fuselage
  { center: [2.1, 0, 0], size: [0.7, 0.32, 0.32] }, // nose
  { center: [-0.3, 0, 0], size: [1.1, 0.12, 5] }, // main wing
  { center: [-1.7, 0.12, 0], size: [0.7, 0.1, 2] }, // tailplane
  { center: [-1.7, 0.5, 0], size: [0.9, 1, 0.1] }, // fin
]

// Box corners are indexed by sign bits: 1 = +X, 2 = +Y, 4 = +Z. Each face lists
// its corners counter-clockwise seen from outside, so normals point outward.
const FACES: ReadonlyArray<{
  corners: readonly [number, number, number, number]
  normal: Vec3
}> = [
  { corners: [1, 3, 7, 5], normal: [1, 0, 0] },
  { corners: [0, 4, 6, 2], normal: [-1, 0, 0] },
  { corners: [2, 6, 7, 3], normal: [0, 1, 0] },
  { corners: [0, 1, 5, 4], normal: [0, -1, 0] },
  { corners: [4, 5, 7, 6], normal: [0, 0, 1] },
  { corners: [0, 2, 3, 1], normal: [0, 0, -1] },
]

let cached: string | null = null

/** A `data:` URI for the generated UAV glTF, built once per session. */
export function uavModelDataUri(): string {
  if (cached !== null) return cached

  const positions: number[] = []
  const normals: number[] = []
  const indices: number[] = []

  for (const part of PARTS) {
    const [cx, cy, cz] = part.center
    const hx = part.size[0] / 2
    const hy = part.size[1] / 2
    const hz = part.size[2] / 2
    const corner = (index: number): Vec3 => [
      cx + (index & 1 ? hx : -hx),
      cy + (index & 2 ? hy : -hy),
      cz + (index & 4 ? hz : -hz),
    ]
    for (const face of FACES) {
      const base = positions.length / 3
      for (const index of face.corners) {
        positions.push(...corner(index))
        normals.push(...face.normal)
      }
      indices.push(base, base + 1, base + 2, base, base + 2, base + 3)
    }
  }

  const positionArray = new Float32Array(positions)
  const normalArray = new Float32Array(normals)
  const indexArray = new Uint16Array(indices)

  const min: [number, number, number] = [Infinity, Infinity, Infinity]
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity]
  for (let i = 0; i < positionArray.length; i += 3) {
    for (let axis = 0; axis < 3; axis += 1) {
      const value = positionArray[i + axis]
      if (value < min[axis]) min[axis] = value
      if (value > max[axis]) max[axis] = value
    }
  }

  const positionBytes = positionArray.byteLength
  const normalBytes = normalArray.byteLength
  const indexBytes = indexArray.byteLength
  const binary = new Uint8Array(positionBytes + normalBytes + indexBytes + (indexBytes % 4))
  binary.set(new Uint8Array(positionArray.buffer, positionArray.byteOffset, positionBytes), 0)
  binary.set(
    new Uint8Array(normalArray.buffer, normalArray.byteOffset, normalBytes),
    positionBytes,
  )
  binary.set(
    new Uint8Array(indexArray.buffer, indexArray.byteOffset, indexBytes),
    positionBytes + normalBytes,
  )

  const gltf = {
    asset: { version: '2.0' },
    scene: 0,
    scenes: [{ nodes: [0] }],
    nodes: [{ mesh: 0 }],
    meshes: [{ primitives: [{ attributes: { POSITION: 0, NORMAL: 1 }, indices: 2, material: 0 }] }],
    materials: [
      {
        pbrMetallicRoughness: {
          baseColorFactor: [1, 1, 1, 1],
          metallicFactor: 0.1,
          roughnessFactor: 0.6,
        },
      },
    ],
    buffers: [
      {
        uri: `data:application/octet-stream;base64,${toBase64(binary)}`,
        byteLength: binary.byteLength,
      },
    ],
    bufferViews: [
      { buffer: 0, byteOffset: 0, byteLength: positionBytes, target: 34962 },
      { buffer: 0, byteOffset: positionBytes, byteLength: normalBytes, target: 34962 },
      { buffer: 0, byteOffset: positionBytes + normalBytes, byteLength: indexBytes, target: 34963 },
    ],
    accessors: [
      {
        bufferView: 0,
        componentType: 5126,
        count: positionArray.length / 3,
        type: 'VEC3',
        min,
        max,
      },
      { bufferView: 1, componentType: 5126, count: normalArray.length / 3, type: 'VEC3' },
      { bufferView: 2, componentType: 5123, count: indexArray.length, type: 'SCALAR' },
    ],
  }

  cached = `data:model/gltf+json;base64,${btoa(JSON.stringify(gltf))}`
  return cached
}

function toBase64(bytes: Uint8Array): string {
  let binary = ''
  const chunkSize = 0x8000
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize))
  }
  return btoa(binary)
}
