// The open project (PJ3): the frontend view over `core::project`'s store,
// exposed by the `project_*` Tauri commands. One project per process — the
// backend's advisory lock enforces it across processes, and this store keeps
// the single handle plus the derived lists the project home renders.
//
// Safety rule (A1 policy): switching or closing a project while the vehicle
// is **armed** is refused with the `armed-vehicle` error code — an armed
// vehicle must not lose its project context. The surveying-state refinement
// lands with M1 (⏸).
//
// Recent projects persist in `localStorage`; the most recent one reopens on
// boot ("restart returns to the last project").

import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import type { Project } from '../generated-types/Project'
import type { Session } from '../generated-types/Session'
import type { Survey } from '../generated-types/Survey'
import { useTelemetryStore } from './telemetry'

const RECENT_KEY = 'maggcs.recentProjects'
const LAST_KEY = 'maggcs.lastProject'
const MAX_RECENT = 5

/** The simplified create form asks for a name only; CRS and vertical datum
 * use these project defaults. */
export const DEFAULT_CRS = 'EPSG:4326'
export const DEFAULT_DATUM = 'cgvd2013'

/** A filesystem-safe folder name for a project (`core::project::is_valid_id`
 * accepts `A-Za-z0-9._-`). */
export function slugifyProjectName(name: string): string {
  const slug = name.trim().replace(/[^A-Za-z0-9._-]+/g, '-').replace(/^[-.]+|[-.]+$/g, '')
  return slug.length > 0 ? slug : 'project'
}

/** The default projects root: `<app data>/projects`. */
export async function defaultProjectsDir(): Promise<string> {
  const { appDataDir, join } = await import('@tauri-apps/api/path')
  return await join(await appDataDir(), 'projects')
}

/** A free sibling directory under `root` for `name` (append `-2`, `-3`, …). */
async function freeProjectDir(root: string, name: string): Promise<string> {
  const { join } = await import('@tauri-apps/api/path')
  const { exists } = await import('@tauri-apps/plugin-fs')
  const base = slugifyProjectName(name)
  let candidate = await join(root, base)
  for (let n = 2; await exists(candidate); n += 1) {
    candidate = await join(root, `${base}-${n}`)
  }
  return candidate
}

export interface RecentProject {
  path: string
  name: string
}

function readRecent(): RecentProject[] {
  try {
    const raw = localStorage.getItem(RECENT_KEY)
    const list: unknown = raw ? JSON.parse(raw) : []
    return Array.isArray(list) ? (list as RecentProject[]) : []
  } catch {
    return []
  }
}

function remember(path: string, name: string): void {
  const next = [{ path, name }, ...readRecent().filter((r) => r.path !== path)].slice(0, MAX_RECENT)
  localStorage.setItem(RECENT_KEY, JSON.stringify(next))
  localStorage.setItem(LAST_KEY, path)
}

/** The vehicle's armed state — the A1 gate for project switch/close. */
function vehicleArmed(): boolean {
  return useTelemetryStore.getState().snapshot?.heartbeat?.base_mode.safety_armed === true
}

interface ProjectState {
  /** The open project's manifest, or `null` when none is open. */
  project: Project | null
  /** Directory of the open project (the lock holder). */
  path: string | null
  /** Surveys of the open project (refreshed on open and after writes). */
  surveys: Survey[]
  /** Error for the panel: a message, or the `armed-vehicle` code. */
  error: string | null
  busy: boolean
  /** In-memory: the operator chose to use the app without opening a project. */
  startDismissed: boolean
  open: (path: string) => Promise<boolean>
  /** Create and open a project. Without `dir` it is created under the default
   * projects root with a unique folder name; an explicit `dir` is used as the
   * project directory. */
  create: (name: string, dir?: string) => Promise<boolean>
  /** Leave the start page with no project open (no disk or IPC change). */
  continueWithoutProject: () => void
  /** Close the project. Refused (false) while the vehicle is armed. */
  close: () => Promise<boolean>
  refreshSurveys: () => Promise<void>
  listSessions: (surveyId: string) => Promise<Session[]>
  saveSurvey: (survey: Survey) => Promise<boolean>
  /** Reopen the most recent project at boot ("restart returns to it"). */
  reopenLast: () => Promise<void>
  recent: () => RecentProject[]
}

export const useProjectStore = create<ProjectState>((set, get) => ({
  project: null,
  path: null,
  surveys: [],
  error: null,
  busy: false,
  startDismissed: false,

  continueWithoutProject: () => set({ startDismissed: true, error: null }),

  open: async (path) => {
    if (vehicleArmed()) {
      set({ error: 'armed-vehicle' })
      return false
    }
    set({ busy: true, error: null })
    try {
      const project = await invoke<Project>('project_open', { path })
      remember(path, project.name)
      set({ project, path, busy: false, startDismissed: false })
      void get().refreshSurveys()
      return true
    } catch (e) {
      set({ error: String(e), busy: false })
      return false
    }
  },

  create: async (name, dir) => {
    if (vehicleArmed()) {
      set({ error: 'armed-vehicle' })
      return false
    }
    set({ busy: true, error: null })
    try {
      const target = dir ?? (await freeProjectDir(await defaultProjectsDir(), name))
      const project = await invoke<Project>('project_create', {
        path: target,
        name,
        crs: DEFAULT_CRS,
        verticalDatum: DEFAULT_DATUM,
      })
      remember(target, project.name)
      set({ project, path: target, busy: false, startDismissed: false })
      void get().refreshSurveys()
      return true
    } catch (e) {
      set({ error: String(e), busy: false })
      return false
    }
  },

  close: async () => {
    if (vehicleArmed()) {
      set({ error: 'armed-vehicle' })
      return false
    }
    set({ busy: true, error: null })
    try {
      await invoke('project_close')
      set({ project: null, path: null, surveys: [], busy: false, startDismissed: false })
      return true
    } catch (e) {
      set({ error: String(e), busy: false })
      return false
    }
  },

  refreshSurveys: async () => {
    try {
      const surveys = await invoke<Survey[]>('project_list_surveys')
      set({ surveys })
    } catch {
      set({ surveys: [] })
    }
  },

  listSessions: async (surveyId) => {
    try {
      return await invoke<Session[]>('project_list_sessions', { surveyId })
    } catch {
      return []
    }
  },

  saveSurvey: async (survey) => {
    try {
      await invoke('project_save_survey', { survey })
      await get().refreshSurveys()
      return true
    } catch (e) {
      set({ error: String(e) })
      return false
    }
  },

  reopenLast: async () => {
    const last = localStorage.getItem(LAST_KEY)
    if (!last || get().project) return
    await get().open(last)
  },

  recent: readRecent,
}))

// Dev-only handle for the browser harness: the forced start page (PJ3) gates
// the shell behind a Tauri-only surface, so the Playwright checks inject a
// throwaway project through this handle (same pattern as `__mgMission`).
if (import.meta.env?.DEV && typeof window !== 'undefined') {
  ;(window as unknown as { __mgProject?: unknown }).__mgProject = useProjectStore
}
