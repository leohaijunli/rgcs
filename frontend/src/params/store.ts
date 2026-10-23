// Parameters window state (plan §4.1/§5.1): FC values + metadata merge,
// user-defined parameter groups (custom sets, separate from the PX4 group
// tree — operator request 2026-10-11), staging area for PM6.

import { create } from 'zustand'
import type { ParamEntry } from '../generated-types/ParamEntry'

/** PX4 metadata entry (tools/param-meta schema). */
export interface ParamMeta {
  name: string
  shortDesc?: string
  longDesc?: string
  group?: string
  category?: string
  type?: string
  default?: number
  min?: number
  max?: number
  increment?: number
  units?: string
  decimalPlaces?: number
  rebootRequired?: boolean
  values?: Array<{ value: number; description: string }>
  bitmask?: Array<{ value: number; description: string }>
}

/** One user-defined group: a named, noted set of parameter names. */
export interface ParamGroup {
  id: string
  name: string
  /** Free-form operator note shown under the group name. */
  note: string
  /** Parameter names (stable across pulls; presence checked at render). */
  members: string[]
}

const GROUPS_KEY = 'maggcs.paramGroups'

function loadGroups(): ParamGroup[] {
  try {
    const raw = localStorage.getItem(GROUPS_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as ParamGroup[]
    if (!Array.isArray(parsed)) return []
    return parsed.filter(
      (g) =>
        typeof g?.id === 'string' &&
        typeof g?.name === 'string' &&
        typeof g?.note === 'string' &&
        Array.isArray(g?.members),
    )
  } catch {
    return []
  }
}

function saveGroups(groups: ParamGroup[]): void {
  try {
    localStorage.setItem(GROUPS_KEY, JSON.stringify(groups))
  } catch {
    /* private mode */
  }
}

let groupSeq = 0
function newGroupId(): string {
  groupSeq += 1
  return `g${Date.now().toString(36)}${groupSeq}`
}

interface ParamsState {
  /** Latest FC values, keyed by name. */
  values: Map<string, ParamEntry>
  /** Pull progress (null when idle). */
  progress: { received: number; total: number } | null
  pulling: boolean
  /** Metadata by name (loaded once at startup). */
  meta: Map<string, ParamMeta>
  /** PX4 metadata groups (name → member names), for the group tree. */
  metaGroups: Map<string, string[]>
  /** User-defined groups (persisted). */
  groups: ParamGroup[]
  /** FC firmware version from AUTOPILOT_VERSION ("" until it arrives). */
  fcVersion: string | null
  /** Staged changes (name → new value); PM6 applies them. */
  staged: Map<string, number | string>

  setMeta: (list: ParamMeta[]) => void
  onEvent: (ev: ParamsIpcEvent) => void
  clearValues: () => void

  addGroup: (name: string, note: string) => void
  renameGroup: (id: string, name: string) => void
  setGroupNote: (id: string, note: string) => void
  deleteGroup: (id: string) => void
  /** Add a parameter to a group (no-op when already a member). */
  addToGroup: (id: string, name: string) => void
  removeFromGroup: (id: string, name: string) => void
}

/** `params_event` payloads (params_service.rs `ParamsEvent`). */
export type ParamsIpcEvent =
  | { kind: 'pull_started'; total: number }
  | { kind: 'batch'; entries: ParamEntry[] }
  | { kind: 'progress'; received: number; total: number }
  | { kind: 'pull_done'; received: number; total: number }
  | { kind: 'external_change'; name: string; value: unknown; ptype: string }
  | { kind: 'fc_version'; version: string }

export const useParamsStore = create<ParamsState>((set, get) => ({
  values: new Map(),
  progress: null,
  pulling: false,
  meta: new Map(),
  metaGroups: new Map(),
  groups: loadGroups(),
  fcVersion: null,
  staged: new Map(),

  setMeta: (list) => {
    const meta = new Map<string, ParamMeta>()
    const metaGroups = new Map<string, string[]>()
    for (const m of list) {
      meta.set(m.name, m)
      const g = m.group ?? 'Uncategorized'
      const arr = metaGroups.get(g) ?? []
      arr.push(m.name)
      metaGroups.set(g, arr)
    }
    set({ meta, metaGroups })
  },

  onEvent: (ev) => {
    if (ev.kind === 'pull_started') {
      set({ values: new Map(), progress: { received: 0, total: ev.total }, pulling: true })
    } else if (ev.kind === 'batch') {
      const values = new Map(get().values)
      for (const e of ev.entries) values.set(e.name, e)
      set({ values })
    } else if (ev.kind === 'progress') {
      set({ progress: { received: ev.received, total: ev.total }, pulling: true })
    } else if (ev.kind === 'pull_done') {
      set({ progress: { received: ev.received, total: ev.total }, pulling: false })
    } else if (ev.kind === 'external_change') {
      const values = new Map(get().values)
      const existing = values.get(ev.name)
      if (existing) values.set(ev.name, existing)
      set({ values })
    } else if (ev.kind === 'fc_version') {
      set({ fcVersion: ev.version })
    }
  },

  clearValues: () => set({ values: new Map(), progress: null, pulling: false }),

  addGroup: (name, note) => {
    const groups = [...get().groups, { id: newGroupId(), name, note, members: [] }]
    saveGroups(groups)
    set({ groups })
  },
  renameGroup: (id, name) => {
    const groups = get().groups.map((g) => (g.id === id ? { ...g, name } : g))
    saveGroups(groups)
    set({ groups })
  },
  setGroupNote: (id, note) => {
    const groups = get().groups.map((g) => (g.id === id ? { ...g, note } : g))
    saveGroups(groups)
    set({ groups })
  },
  deleteGroup: (id) => {
    const groups = get().groups.filter((g) => g.id !== id)
    saveGroups(groups)
    set({ groups })
  },
  addToGroup: (id, name) => {
    const groups = get().groups.map((g) =>
      g.id === id && !g.members.includes(name) ? { ...g, members: [...g.members, name] } : g,
    )
    saveGroups(groups)
    set({ groups })
  },
  removeFromGroup: (id, name) => {
    const groups = get().groups.map((g) =>
      g.id === id ? { ...g, members: g.members.filter((m) => m !== name) } : g,
    )
    saveGroups(groups)
    set({ groups })
  },
}))

// Dev-only handle for inspecting the app from the DevTools protocol / E2E
// scripts (same pattern as `window.__mgMission` in stores/mission.ts).
if (import.meta.env?.DEV && typeof window !== 'undefined') {
  ;(window as unknown as { __mgParams?: unknown }).__mgParams = useParamsStore
}