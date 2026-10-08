// QGroundControl `.plan` file import/export (Phase 1).
//
// The `.plan` file is a JSON document (QGC JSON Plan format v1). We map the
// `mission.items` array to/from our `MissionItem` representation, keep the rest
// of the document as an opaque base so export is lossless, and use the
// mission's global altitude mode to seed the altitude-mode selector.

import { readTextFile, writeTextFile } from '@tauri-apps/plugin-fs'
import { open, save } from '@tauri-apps/plugin-dialog'
import type { MissionItem } from '../generated-types/MissionItem'
import type { MissionFrame } from '../generated-types/MissionFrame'
import type { AltitudeMode } from '../stores/mission'

interface QgcItem {
  autoContinue?: boolean
  command: number
  doJumpId?: number
  frame: number
  params?: number[]
  coordinate?: [number, number, number]
  type?: string
  /** Present on newer QGC complex items: the generated simple waypoints. */
  simpleItems?: QgcItem[]
  [key: string]: unknown
}

export interface QgcPlan {
  fileType?: string
  version?: number
  geoFence?: unknown
  rallyPoints?: unknown
  mission?: {
    items?: QgcItem[]
    globalPlanAltitudeMode?: number
    plannedHomePosition?: [number, number, number]
    [key: string]: unknown
  }
  [key: string]: unknown
}

/** Result of parsing a `.plan` document. */
/**
 * A QGC *complex* item (Survey, Corridor Scan, ...). Its generated waypoints are
 * flown exactly like simple items, but the block itself carries the mission
 * parameters (polygon, spacing, angle) that cannot be reconstructed from the
 * waypoints. It is therefore kept verbatim and written back unchanged on
 * export, so a QGC survey survives a round trip.
 */
export interface PlanBlock {
  /** QGC type of the complex item, e.g. `Survey`. */
  type: string
  /** Position of the item in the source `mission.items` array. */
  index: number
  /** Original JSON, written back unchanged. */
  raw: QgcItem
  /** Waypoints generated from the block; read-only in the editor. */
  children: MissionItem[]
}

export interface PlanImport {
  /** Editable (simple) items. Complex-item waypoints live in `blocks`. */
  items: MissionItem[]
  mode: AltitudeMode
  /** `plannedHomePosition`, if the file declared one. */
  home: [number, number, number] | null
  /** Original document, kept so export can write back unknown fields. */
  base: QgcPlan
  /** Complex items kept as opaque, non-editable blocks. */
  blocks: PlanBlock[]
  /** Human-readable labels of items that could not be converted. */
  unsupported: string[]
}

const QGC_FILTER = [{ name: 'QGroundControl Plan', extensions: ['plan'] }]

/** Home used only when a plan has neither a declared home nor any waypoint. */
const FALLBACK_HOME: [number, number, number] = [48.6493, -123.3982, 5]

/**
 * Synthetic `seq` base for complex-item child waypoints. Child seqs must not
 * collide with editable items (which use 0..n-1 and are resequenced by the
 * store), because the map keys entities by `wp-<seq>` and a Map cannot hold two
 * entries for one seq. The `seq` value is UI-only: upload order is array order.
 */
const BLOCK_SEQ_BASE = 1_000_000

const MAV_FRAME_TO_QGC: Record<MissionFrame, number> = {
  local_ned: 1,
  local_enu: 4,
  global_int: 5,
  global_relative_alt_int: 6,
  local_offset_ned: 7,
  body_ned: 8,
  global_terrain_alt_int: 11,
}

const QGC_FRAME_TO_MAV: Record<number, MissionFrame> = {
  // QGC writes the non-INT MAV_FRAME spellings too; we normalise them to the
  // INT frame we actually exchange (issues.md #11).
  0: 'global_int',
  1: 'local_ned',
  3: 'global_relative_alt_int',
  4: 'local_enu',
  5: 'global_int',
  6: 'global_relative_alt_int',
  7: 'local_offset_ned',
  8: 'body_ned',
  10: 'global_terrain_alt_int',
  11: 'global_terrain_alt_int',
}

const QGC_ALT_MODE_TO_OURS: Record<number, AltitudeMode> = {
  0: 'amsl',
  1: 'relative',
  2: 'agl',
}

const OURS_ALT_MODE_TO_QGC: Record<AltitudeMode, number> = {
  amsl: 0,
  relative: 1,
  agl: 2,
}

function isCoordinate(v: unknown): v is [number, number, number] {
  return (
    Array.isArray(v) &&
    v.length >= 3 &&
    v.slice(0, 3).every((n) => typeof n === 'number' && Number.isFinite(n))
  )
}

/** QGC writes unset params (e.g. yaw) as `null`; Rust `f32` rejects those. */
function finiteOrZero(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

/** Convert one QGC simple item, or `null` if it carries no usable coordinate. */
function simpleToItem(raw: QgcItem, seq: number): MissionItem | null {
  if (!isCoordinate(raw.coordinate)) return null
  const [lat, lon, alt] = raw.coordinate
  // Only P1..P4 are modelled; P5/P6/P7 are the coordinate fields and live in
  // `x`/`y`/`z` (issues.md #10).
  const params = (raw.params ?? []).slice(0, 4).map(finiteOrZero)
  while (params.length < 4) params.push(0)
  return {
    seq,
    frame: QGC_FRAME_TO_MAV[raw.frame] ?? 'global_relative_alt_int',
    command: raw.command,
    params,
    x: Math.round(lat * 1e7),
    y: Math.round(lon * 1e7),
    z: alt,
    autocontinue: raw.autoContinue ?? true,
    current: seq === 0,
  }
}

function itemToQgc(item: MissionItem, doJumpId: number): QgcItem {
  const lat = item.x / 1e7
  const lon = item.y / 1e7
  const alt = item.z
  // QGC writes all 7 params; P5/P6/P7 mirror the coordinate for global frames.
  const params = item.params.slice(0, 4)
  while (params.length < 4) params.push(0)
  params.push(lat, lon, alt)
  return {
    autoContinue: item.autocontinue,
    command: item.command,
    doJumpId,
    frame: MAV_FRAME_TO_QGC[item.frame] ?? 6,
    params,
    type: 'SimpleItem',
    coordinate: [lat, lon, alt],
  }
}

function labelOf(raw: QgcItem, index: number): string {
  const kind = raw.type && raw.type !== 'SimpleItem' ? raw.type : 'item'
  return `${kind} #${index + 1}`
}

/** Parse the contents of a `.plan` file. */
export function parsePlan(json: string): PlanImport {
  const plan = JSON.parse(json) as QgcPlan
  const mission = plan.mission ?? {}
  const items: MissionItem[] = []
  const blocks: PlanBlock[] = []
  const unsupported: string[] = []

  ;(mission.items ?? []).forEach((raw, i) => {
    if (isCoordinate(raw.coordinate)) {
      const item = simpleToItem(raw, items.length)
      if (item) items.push(item)
      else unsupported.push(labelOf(raw, i))
      return
    }
    // QGC complex items (Survey, Corridor Scan, Structure Scan) have no
    // `coordinate`. Their generated children are flown, but the block itself
    // holds the survey parameters, so keep it verbatim (issues.md #12) and
    // collect the children as read-only waypoints.
    const children: MissionItem[] = []
    if (Array.isArray(raw.simpleItems)) {
      raw.simpleItems.forEach((child, c) => {
        const seq = BLOCK_SEQ_BASE + blocks.length * 1000 + c
        const item = simpleToItem(child, seq)
        if (item) children.push({ ...item, current: false })
      })
    }
    if (children.length > 0 || raw.type) {
      blocks.push({
        type: typeof raw.type === 'string' ? raw.type : 'ComplexItem',
        index: i,
        raw,
        children,
      })
    } else {
      unsupported.push(labelOf(raw, i))
    }
  })

  items.forEach((item, idx) => {
    item.current = idx === 0
  })

  const qgcMode = mission.globalPlanAltitudeMode ?? 1
  const home = isCoordinate(mission.plannedHomePosition)
    ? [...mission.plannedHomePosition]
    : null
  return {
    items,
    mode: QGC_ALT_MODE_TO_OURS[qgcMode] ?? 'relative',
    home: home as [number, number, number] | null,
    base: plan,
    blocks,
    unsupported,
  }
}

function deriveHome(items: MissionItem[]): [number, number, number] {
  if (items.length > 0) {
    return [items[0].x / 1e7, items[0].y / 1e7, items[0].z]
  }
  return FALLBACK_HOME
}

/**
 * Flyable order of a plan: editable items and complex-item waypoints merged the
 * way the source file ordered them (`base.mission.items`), with user-added
 * items appended. The map, the upload and the export all use this order.
 */
export function orderedMissionItems(
  items: MissionItem[],
  blocks: PlanBlock[] = [],
  base?: QgcPlan,
): MissionItem[] {
  if (blocks.length === 0) return items
  const skeleton = base?.mission?.items
  if (!Array.isArray(skeleton)) return [...items, ...blocks.flatMap((b) => b.children)]
  const byIndex = new Map(blocks.map((b) => [b.index, b]))
  const queue = [...items]
  const out: MissionItem[] = []
  skeleton.forEach((_entry, i) => {
    const block = byIndex.get(i)
    if (block) {
      out.push(...block.children)
      return
    }
    const next = queue.shift()
    if (next) out.push(next)
  })
  out.push(...queue)
  return out
}

/** Highest `doJumpId` anywhere in a plan, including complex-item children. */
function maxDoJumpId(entries: unknown): number {
  let max = 0
  const walk = (list: unknown) => {
    if (!Array.isArray(list)) return
    list.forEach((entry) => {
      const item = entry as QgcItem
      if (typeof item?.doJumpId === 'number' && item.doJumpId > max) max = item.doJumpId
      walk(item?.simpleItems)
    })
  }
  walk(entries)
  return max
}

/** Serialize mission items into a `.plan` document. */
export function buildPlan(
  items: MissionItem[],
  mode: AltitudeMode,
  opts: { base?: QgcPlan; home?: [number, number, number] | null; blocks?: PlanBlock[] } = {},
): string {
  const base = opts.base ? structuredClone(opts.base) : ({ fileType: 'Plan', version: 1 } as QgcPlan)
  const baseMission = base.mission ?? {}
  const home = opts.home ?? baseMission.plannedHomePosition ?? deriveHome(items)
  const blocks = opts.blocks ?? []
  const skeleton = Array.isArray(baseMission.items) ? baseMission.items : undefined
  // Complex items keep their original doJumpIds, so number *appended* simple
  // items above every id already in the file to avoid collisions.
  let appendedJumpId = maxDoJumpId([skeleton, blocks.map((b) => b.raw)]) + 1
  let outItems: QgcItem[]
  if (blocks.length > 0 && skeleton) {
    // Rebuild the file in its original order: complex items verbatim, simple
    // entries replaced by the current items (issues.md #12, WS-A5). Reusing the
    // source doJumpId keeps an unedited round trip structurally identical.
    const byIndex = new Map(blocks.map((b) => [b.index, b]))
    const queue = [...items]
    outItems = []
    skeleton.forEach((entry, i) => {
      const block = byIndex.get(i)
      if (block) {
        outItems.push(block.raw)
        return
      }
      const next = queue.shift()
      if (next) outItems.push(itemToQgc(next, entry.doJumpId ?? i + 1))
    })
    queue.forEach((item) => outItems.push(itemToQgc(item, appendedJumpId++)))
  } else {
    outItems = items.map((item, i) => itemToQgc(item, i + 1))
    blocks.forEach((block) => outItems.push(block.raw))
  }
  const plan: QgcPlan = {
    ...base,
    fileType: base.fileType ?? 'Plan',
    version: base.version ?? 1,
    // Keep whatever the imported file had (geoFence, rallyPoints, ...).
    geoFence: base.geoFence ?? { circles: [], polygons: [] },
    rallyPoints: base.rallyPoints ?? { points: [] },
    mission: {
      cruiseSpeed: 15,
      hoverSpeed: 3,
      firmwareType: 3,
      vehicleType: 2,
      ...baseMission,
      plannedHomePosition: home,
      globalPlanAltitudeMode: OURS_ALT_MODE_TO_QGC[mode],
      items: outItems,
    },
  }
  return JSON.stringify(plan, null, 2)
}

/** Show an open dialog, parse the chosen `.plan`, and return items + mode. */
export async function importPlanFile(): Promise<PlanImport | null> {
  const path = await open({ multiple: false, filters: QGC_FILTER })
  if (typeof path !== 'string') return null
  const text = await readTextFile(path)
  return parsePlan(text)
}

/** Show a save dialog and write the `.plan` document. */
export async function exportPlanFile(
  items: MissionItem[],
  mode: AltitudeMode,
  opts: { base?: QgcPlan; home?: [number, number, number] | null; blocks?: PlanBlock[] } = {},
): Promise<boolean> {
  const path = await save({ defaultPath: 'mission.plan', filters: QGC_FILTER })
  if (typeof path !== 'string') return false
  await writeTextFile(path, buildPlan(items, mode, opts))
  return true
}
