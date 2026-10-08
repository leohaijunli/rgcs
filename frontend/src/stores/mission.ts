// Mission planning state: waypoint list, altitude mode, and the upload /
// download / clear operations driven through the desktop mission service.

import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import type { MissionItem } from '../generated-types/MissionItem'
import type { MissionFrame } from '../generated-types/MissionFrame'
import { orderedMissionItems } from '../mission/planfile'
import { convertAltitudeZ } from '../mission/altitude'
import { itemsHash } from '../mission/hash'
import type { PlanBlock, PlanImport, QgcPlan } from '../mission/planfile'

export type AltitudeMode = 'relative' | 'amsl' | 'agl'

/** Payload of the backend `mission` event. */
export interface MissionEventPayload {
  op: 'upload' | 'download' | 'clear' | 'set_current' | 'mission'
  kind: 'progress' | 'completed' | 'current_changed' | 'failed'
  sent: number
  total: number
  seq: number
  message?: string | null
}

export const FRAME_BY_MODE: Record<AltitudeMode, MissionFrame> = {
  relative: 'global_relative_alt_int',
  amsl: 'global_int',
  agl: 'global_terrain_alt_int',
}

interface MissionState {
  items: MissionItem[]
  selectedSeq: number | null
  altitudeMode: AltitudeMode
  busy: boolean
  lastEvent: MissionEventPayload | null
  syncState: 'idle' | 'uploading' | 'downloading' | 'clearing'
  /** True when the plan was edited since it was last known to match the FC. */
  dirty: boolean
  /** Hash of the items at the last successful sync (upload or download). */
  lastSyncedHash: string | null
  /** Result of the post-upload read-back: null = unknown/not checked. */
  fcMatches: boolean | null
  /** True while a read-back download is in flight. */
  verifying: boolean
  /** Original imported `.plan`, kept so export can write back unknown fields. */
  planBase: QgcPlan | null
  /** Complex items (Survey, ...) kept opaque and read-only (issues.md #12). */
  blocks: PlanBlock[]
  /** `plannedHomePosition` from the imported plan, if any. */
  home: [number, number, number] | null
  select: (seq: number | null) => void
  setItems: (items: MissionItem[]) => void
  setAltitudeMode: (mode: AltitudeMode, opts?: { convert?: boolean }) => void
  updateItem: (seq: number, patch: Partial<MissionItem>) => void
  addWaypoint: (latDeg: number, lonDeg: number, altM: number) => void
  removeWaypoint: (seq: number) => void
  moveWaypoint: (from: number, to: number) => void
  upload: () => Promise<void>
  download: () => Promise<void>
  clear: () => Promise<void>
  setCurrent: (seq: number) => Promise<void>
  handleEvent: (e: MissionEventPayload) => void
  handlePlan: (items: MissionItem[]) => void
  /** Editable items plus complex-item children, in flyable order. */
  flyable: () => MissionItem[]
  applyImport: (result: PlanImport) => void
  verifyFc: () => Promise<void>
  reset: () => void
}

function resequence(items: MissionItem[]): MissionItem[] {
  return items.map((it, i) => ({ ...it, seq: i }))
}

export { itemsHash } from '../mission/hash'

function useSelection(items: MissionItem[], selectedSeq: number | null): number | null {
  if (selectedSeq === null) return null
  return items.some((it) => it.seq === selectedSeq) ? selectedSeq : null
}

export const useMissionStore = create<MissionState>((set, get) => ({
  items: [],
  selectedSeq: null,
  altitudeMode: 'relative',
  busy: false,
  lastEvent: null,
  syncState: 'idle',
  dirty: false,
  lastSyncedHash: null,
  fcMatches: null,
  verifying: false,
  planBase: null,
  blocks: [],
  home: null,

  select: (seq) => set({ selectedSeq: seq }),

  setItems: (items) =>
    set((s) => ({
      items,
      selectedSeq: useSelection(items, s.selectedSeq),
      dirty: true,
      fcMatches: null,
    })),

  setAltitudeMode: (mode, opts) =>
    set((s) => {
      if (mode === s.altitudeMode) return s
      // Complex-item children keep the frame stored in the file, so only the
      // editable items can be converted here (finding 1).
      if (!opts?.convert || s.items.length === 0) return { altitudeMode: mode }
      const anchor = { homeAmslM: s.home?.[2] ?? 0 }
      const items = s.items.map((it) => ({
        ...it,
        z: convertAltitudeZ(it.z, s.altitudeMode, mode, anchor),
        frame: FRAME_BY_MODE[mode],
      }))
      return { altitudeMode: mode, items, dirty: true, fcMatches: null }
    }),

  updateItem: (seq, patch) =>
    set((s) => ({
      items: s.items.map((it) => (it.seq === seq ? { ...it, ...patch } : it)),
      dirty: true,
      fcMatches: null,
    })),

  addWaypoint: (latDeg, lonDeg, altM) =>
    set((s) => {
      // Continue past every existing seq so a new item never shares an id with
      // an imported complex-item child (which uses a high synthetic seq base).
      const nextSeq = s.items.reduce((m, it) => Math.max(m, it.seq + 1), 0)
      const item: MissionItem = {
        seq: nextSeq,
        frame: FRAME_BY_MODE[s.altitudeMode],
        command: 16, // MAV_CMD_NAV_WAYPOINT
        params: [0, 0, 0, 0],
        x: Math.round(latDeg * 1e7),
        y: Math.round(lonDeg * 1e7),
        z: altM,
        autocontinue: true,
        current: s.items.length === 0,
      }
      return { items: [...s.items, item], selectedSeq: item.seq, dirty: true, fcMatches: null }
    }),

  removeWaypoint: (seq) =>
    set((s) => ({
      items: resequence(s.items.filter((it) => it.seq !== seq)),
      selectedSeq: null,
      dirty: true,
      fcMatches: null,
    })),

  moveWaypoint: (from, to) =>
    set((s) => {
      const items = [...s.items]
      if (from < 0 || from >= items.length || to < 0 || to >= items.length) return s
      const [moved] = items.splice(from, 1)
      items.splice(to, 0, moved)
      const next = resequence(items)
      return {
        items: next,
        selectedSeq: useSelection(next, s.selectedSeq),
        dirty: true,
        fcMatches: null,
      }
    }),

  upload: async () => {
    const items = get().flyable()
    if (items.length === 0) return
    set({ busy: true, syncState: 'uploading', fcMatches: null })
    try {
      await invoke('mission_upload', { items })
    } catch (err) {
      set({
        busy: false,
        syncState: 'idle',
        lastEvent: {
          op: 'mission',
          kind: 'failed',
          sent: 0,
          total: 0,
          seq: 0,
          message: String(err),
        },
      })
    }
  },

  download: async () => {
    set({ busy: true, syncState: 'downloading', verifying: false })
    try {
      await invoke('mission_download')
    } catch (err) {
      set({
        busy: false,
        syncState: 'idle',
        lastEvent: {
          op: 'mission',
          kind: 'failed',
          sent: 0,
          total: 0,
          seq: 0,
          message: String(err),
        },
      })
    }
  },

  clear: async () => {
    set({ busy: true, syncState: 'clearing' })
    try {
      await invoke('mission_clear')
    } catch (err) {
      set({
        busy: false,
        syncState: 'idle',
        lastEvent: {
          op: 'mission',
          kind: 'failed',
          sent: 0,
          total: 0,
          seq: 0,
          message: String(err),
        },
      })
    }
  },

  setCurrent: async (seq) => {
    try {
      await invoke('mission_set_current', { seq })
    } catch {
      /* best-effort */
    }
  },

  handleEvent: (e) => {
    set((s) => {
      const done =
        e.kind === 'completed' || e.kind === 'failed'
      const busy = done ? false : s.busy
      const syncState: MissionState['syncState'] = done
        ? 'idle'
        : e.op === 'upload' && e.kind === 'progress'
          ? 'uploading'
          : e.op === 'download' && e.kind === 'progress'
            ? 'downloading'
            : s.syncState
      const patch: Partial<MissionState> = { lastEvent: e, busy, syncState }
      if (e.kind === 'completed' && e.op === 'upload') {
        // We believe the FC now holds exactly our plan; confirm with a
        // read-back download (issues.md #15).
        patch.dirty = false
        patch.lastSyncedHash = itemsHash(
          orderedMissionItems(s.items, s.blocks, s.planBase ?? undefined),
        )
        patch.fcMatches = true
      }
      return patch
    })
    if (e.kind === 'completed' && e.op === 'upload') {
      void get().verifyFc()
    }
  },

  handlePlan: (items) => {
    set((s) => ({
      ...(s.verifying
        ? {
            verifying: false,
            fcMatches: itemsHash(items) === s.lastSyncedHash,
            busy: false,
            syncState: 'idle' as const,
          }
        : {
            items,
            blocks: [],
            planBase: null,
            dirty: false,
            lastSyncedHash: itemsHash(items),
            fcMatches: true,
            busy: false,
            syncState: 'idle' as const,
            selectedSeq: useSelection(items, s.selectedSeq),
          }),
    }))
  },

  applyImport: (result) =>
    set((s) => ({
      items: result.items,
      altitudeMode: result.mode,
      selectedSeq: useSelection(result.items, s.selectedSeq),
      dirty: true,
      fcMatches: null,
      planBase: result.base,
      blocks: result.blocks,
      home: result.home,
    })),

  flyable: () => {
    const s = get()
    return orderedMissionItems(s.items, s.blocks, s.planBase ?? undefined)
  },

  verifyFc: async () => {
    if (!get().lastSyncedHash) return
    set({ verifying: true })
    try {
      await invoke('mission_download')
    } catch {
      set({ verifying: false, fcMatches: false })
    }
  },

  reset: () =>
    set({
      items: [],
      selectedSeq: null,
      busy: false,
      lastEvent: null,
      syncState: 'idle',
      dirty: false,
      lastSyncedHash: null,
      fcMatches: null,
      verifying: false,
      planBase: null,
      blocks: [],
      home: null,
    }),
}))

/** Helper: convert MAVLink int lat/lon (×1e7) to degrees. */
export function degFromMavInt(v: number): number {
  return v / 1e7
}

/** Helper: convert degrees to the MAVLink int coordinate. */
export function mavIntFromDeg(v: number): number {
  return Math.round(v * 1e7)
}
