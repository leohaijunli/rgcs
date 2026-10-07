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
export interface PlanImport {
  items: MissionItem[]
  mode: AltitudeMode
  /** `plannedHomePosition`, if the file declared one. */
  home: [number, number, number] | null
  /** Original document, kept so export can write back unknown fields. */
  base: QgcPlan
  /** Human-readable labels of items that could not be converted. */
  unsupported: string[]
}

const QGC_FILTER = [{ name: 'QGroundControl Plan', extensions: ['plan'] }]

/** Home used only when a plan has neither a declared home nor any waypoint. */
const FALLBACK_HOME: [number, number, number] = [48.6493, -123.3982, 5]

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

/** Convert one QGC simple item, or `null` if it carries no usable coordinate. */
function simpleToItem(raw: QgcItem, seq: number): MissionItem | null {
  if (!isCoordinate(raw.coordinate)) return null
  const [lat, lon, alt] = raw.coordinate
  // Only P1..P4 are modelled; P5/P6/P7 are the coordinate fields and live in
  // `x`/`y`/`z` (issues.md #10).
  const params = (raw.params ?? []).slice(0, 4)
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
  const unsupported: string[] = []

  ;(mission.items ?? []).forEach((raw, i) => {
    if (isCoordinate(raw.coordinate)) {
      const item = simpleToItem(raw, items.length)
      if (item) items.push(item)
      else unsupported.push(labelOf(raw, i))
      return
    }
    // QGC complex items (Survey, Corridor Scan, Structure Scan) have no
    // `coordinate`; use their generated simple children when present,
    // otherwise report them rather than throwing (issues.md #12).
    if (Array.isArray(raw.simpleItems)) {
      raw.simpleItems.forEach((child) => {
        const item = simpleToItem(child, items.length)
        if (item) items.push(item)
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
    unsupported,
  }
}

function deriveHome(items: MissionItem[]): [number, number, number] {
  if (items.length > 0) {
    return [items[0].x / 1e7, items[0].y / 1e7, items[0].z]
  }
  return FALLBACK_HOME
}

/** Serialize mission items into a `.plan` document. */
export function buildPlan(
  items: MissionItem[],
  mode: AltitudeMode,
  opts: { base?: QgcPlan; home?: [number, number, number] | null } = {},
): string {
  const base = opts.base ? structuredClone(opts.base) : ({ fileType: 'Plan', version: 1 } as QgcPlan)
  const baseMission = base.mission ?? {}
  const home = opts.home ?? baseMission.plannedHomePosition ?? deriveHome(items)
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
      items: items.map((item, i) => itemToQgc(item, i + 1)),
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
  opts: { base?: QgcPlan; home?: [number, number, number] | null } = {},
): Promise<boolean> {
  const path = await save({ defaultPath: 'mission.plan', filters: QGC_FILTER })
  if (typeof path !== 'string') return false
  await writeTextFile(path, buildPlan(items, mode, opts))
  return true
}
