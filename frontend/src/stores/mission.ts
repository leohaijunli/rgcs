// Mission planning state: waypoint list, altitude mode, and the upload /
// download / clear operations driven through the desktop mission service.

import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import type { MissionItem } from '../generated-types/MissionItem'
import type { MissionFrame } from '../generated-types/MissionFrame'

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
  select: (seq: number | null) => void
  setItems: (items: MissionItem[]) => void
  setAltitudeMode: (mode: AltitudeMode) => void
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
  reset: () => void
}

function resequence(items: MissionItem[]): MissionItem[] {
  return items.map((it, i) => ({ ...it, seq: i }))
}

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

  select: (seq) => set({ selectedSeq: seq }),

  setItems: (items) =>
    set((s) => ({
      items,
      selectedSeq: useSelection(items, s.selectedSeq),
    })),

  setAltitudeMode: (mode) => set({ altitudeMode: mode }),

  updateItem: (seq, patch) =>
    set((s) => ({
      items: s.items.map((it) => (it.seq === seq ? { ...it, ...patch } : it)),
    })),

  addWaypoint: (latDeg, lonDeg, altM) =>
    set((s) => {
      const item: MissionItem = {
        seq: s.items.length,
        frame: FRAME_BY_MODE[s.altitudeMode],
        command: 16, // MAV_CMD_NAV_WAYPOINT
        params: [0, 0, 0, 0, 0, 0, 0],
        x: Math.round(latDeg * 1e7),
        y: Math.round(lonDeg * 1e7),
        z: altM,
        autocontinue: true,
        current: s.items.length === 0,
      }
      return { items: [...s.items, item], selectedSeq: item.seq }
    }),

  removeWaypoint: (seq) =>
    set((s) => ({
      items: resequence(s.items.filter((it) => it.seq !== seq)),
      selectedSeq: null,
    })),

  moveWaypoint: (from, to) =>
    set((s) => {
      const items = [...s.items]
      if (from < 0 || from >= items.length || to < 0 || to >= items.length) return s
      const [moved] = items.splice(from, 1)
      items.splice(to, 0, moved)
      const next = resequence(items)
      return { items: next, selectedSeq: useSelection(next, s.selectedSeq) }
    }),

  upload: async () => {
    const { items } = get()
    if (items.length === 0) return
    set({ busy: true, syncState: 'uploading' })
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
    set({ busy: true, syncState: 'downloading' })
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
      return { lastEvent: e, busy, syncState }
    })
  },

  handlePlan: (items) => {
    set((s) => ({
      items,
      busy: false,
      syncState: 'idle',
      selectedSeq: useSelection(items, s.selectedSeq),
    }))
  },

  reset: () =>
    set({
      items: [],
      selectedSeq: null,
      busy: false,
      lastEvent: null,
      syncState: 'idle',
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