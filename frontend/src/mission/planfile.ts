// QGroundControl `.plan` file import/export (Phase 1).
//
// The `.plan` file is a JSON document (QGC JSON Plan format v1). We map the
// `mission.items` array to/from our `MissionItem` representation and use the
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
  params: number[]
  coordinate: [number, number, number]
  type?: string
}

interface QgcPlan {
  fileType?: string
  version?: number
  mission?: {
    items?: QgcItem[]
    globalPlanAltitudeMode?: number
    plannedHomePosition?: [number, number, number]
    [key: string]: unknown
  }
  [key: string]: unknown
}

const QGC_FILTER = [{ name: 'QGroundControl Plan', extensions: ['plan'] }]

const MAV_FRAME_TO_QGC: Record<MissionFrame, number> = {
  global: 0,
  local_ned: 1,
  global_relative_alt: 3,
  local_enu: 4,
  global_int: 5,
  global_relative_alt_int: 6,
  local_offset_ned: 7,
  body_ned: 8,
  global_terrain_alt: 10,
  global_terrain_alt_int: 11,
}

const QGC_FRAME_TO_MAV: Record<number, MissionFrame> = {
  0: 'global',
  1: 'local_ned',
  3: 'global_relative_alt',
  4: 'local_enu',
  5: 'global_int',
  6: 'global_relative_alt_int',
  7: 'local_offset_ned',
  8: 'body_ned',
  10: 'global_terrain_alt',
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

function qgcToItem(raw: QgcItem, seq: number): MissionItem {
  const [lat, lon, alt] = raw.coordinate
  const params = [...raw.params]
  while (params.length < 7) params.push(0)
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

function itemToQgc(item: MissionItem): QgcItem {
  const lat = item.x / 1e7
  const lon = item.y / 1e7
  const alt = item.z
  const params = [...item.params]
  while (params.length < 7) params.push(0)
  params[4] = lat
  params[5] = lon
  params[6] = alt
  return {
    autoContinue: item.autocontinue,
    command: item.command,
    doJumpId: 0,
    frame: MAV_FRAME_TO_QGC[item.frame] ?? 6,
    params,
    type: 'SimpleItem',
    coordinate: [lat, lon, alt],
  }
}

/** Parse the contents of a `.plan` file into mission items + altitude mode. */
export function parsePlan(json: string): { items: MissionItem[]; mode: AltitudeMode } {
  const plan = JSON.parse(json) as QgcPlan
  const mission = plan.mission ?? {}
  const items = (mission.items ?? []).map((raw, i) => qgcToItem(raw, i))
  const qgcMode = mission.globalPlanAltitudeMode ?? 1
  return { items, mode: QGC_ALT_MODE_TO_OURS[qgcMode] ?? 'relative' }
}

/** Serialize mission items into a `.plan` document. */
export function buildPlan(items: MissionItem[], mode: AltitudeMode): string {
  const qgcItems = items.map(itemToQgc)
  const home =
    items.length > 0
      ? ([items[0].x / 1e7, items[0].y / 1e7, items[0].z] as [number, number, number])
      : ([48.6493, -123.3982, 5] as [number, number, number])
  const plan: QgcPlan = {
    fileType: 'Plan',
    version: 1,
    geoFence: { circles: [], polygons: [] },
    rallyPoints: { points: [] },
    mission: {
      cruiseSpeed: 15,
      hoverSpeed: 3,
      plannedHomePosition: home,
      firmwareType: 3,
      globalPlanAltitudeMode: OURS_ALT_MODE_TO_QGC[mode],
      vehicleType: 2,
      items: qgcItems,
    },
  }
  return JSON.stringify(plan, null, 2)
}

/** Show an open dialog, parse the chosen `.plan`, and return items + mode. */
export async function importPlanFile(): Promise<{ items: MissionItem[]; mode: AltitudeMode } | null> {
  const path = await open({ multiple: false, filters: QGC_FILTER })
  if (typeof path !== 'string') return null
  const text = await readTextFile(path)
  return parsePlan(text)
}

/** Show a save dialog and write the `.plan` document. */
export async function exportPlanFile(items: MissionItem[], mode: AltitudeMode): Promise<boolean> {
  const path = await save({ defaultPath: 'mission.plan', filters: QGC_FILTER })
  if (typeof path !== 'string') return false
  await writeTextFile(path, buildPlan(items, mode))
  return true
}