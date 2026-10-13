// Mission planning state: planned waypoints, altitude mode, and the upload /
// download / clear operations driven through the desktop mission service.
//
// Waypoints are stored in absolute AMSL (ADR-013); the wire frame is chosen at
// compile time by `mission/compile.ts`. That keeps a mode switch lossless: it
// only changes how the plan compiles, never what the plan means.

import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import type { GeoPoint } from '../generated-types/GeoPoint'
import type { MissionItem } from '../generated-types/MissionItem'
import type { PatternLine } from '../generated-types/PatternLine'
import type { PatternStats } from '../generated-types/PatternStats'
import type { DrapeReport } from '../generated-types/DrapeReport'
import type { PatternPlan } from '../generated-types/PatternPlan'
import type { PlannedWaypoint } from '../generated-types/PlannedWaypoint'
import {
  compileWaypoints,
  DEFAULT_ALT_AGL_M,
  makeWaypoint,
  modeFromFrame,
  waypointFromItem,
} from '../mission/compile'
import { orderedMissionItems } from '../mission/planfile'
import { itemsHash } from '../mission/hash'
import type { PlanBlock, PlanImport, QgcPlan } from '../mission/planfile'
import { useTerrainStore } from './terrain'

/**
 * Altitude semantics of the editable plan.
 * - `relative`  — z stored as AMSL, flown as HOME-relative (compile converts).
 * - `amsl`      — z stored and flown as AMSL.
 * - `agl`       — z stored as AMSL; the HUD/labels show height − HOME AMSL.
 * - `terrain`   — terrain following (ADR-005/013 addendum): the Pattern
 *   altitude is a **survey AGL**; every plan change is automatically draped
 *   over the loaded DEM (DTM + AGL, raised above the DSM clearance), so the
 *   stored waypoints stay absolute AMSL (ADR-013 discipline) while always
 *   hugging the terrain. Without a loaded DEM the plan keeps plain heights
 *   and the panel says so.
 */
export type AltitudeMode = 'relative' | 'amsl' | 'agl' | 'terrain'

/**
 * The most recently inserted preset pattern, for the line readout and for
 * re-generating: while it is set the trailing `count` waypoints are exactly the
 * block it produced, so Generate replaces them instead of appending a duplicate.
 * Any manual edit clears it. `center` remembers where the pattern was placed so
 * re-generating after a view switch does not silently follow the new map centre.
 */
export interface InsertedPattern {
  label: string
  count: number
  lines: PatternLine[]
  /** Centre the pattern was generated from (cloverleaf centre / rectangle centre). */
  center: GeoPoint | null
  /** Quality metrics from the S1/S2 compiler, when compiled. */
  stats: PatternStats | null
}

/** Result of the terrain drape (S3/D5) kept for the plan readout. */
export interface DrapeState {
  report: DrapeReport
  /** The dem-prep tile folder the drape was computed against. */
  folder: string
}

/** Payload of the backend `mission` event. */
export interface MissionEventPayload {
  op: 'upload' | 'download' | 'clear' | 'set_current' | 'mission'
  kind: 'progress' | 'completed' | 'current_changed' | 'failed'
  sent: number
  total: number
  seq: number
  message?: string | null
}

interface MissionState {
  /** Editable waypoints, stored in AMSL (ADR-013). */
  waypoints: PlannedWaypoint[]
  selectedSeq: number | null
  altitudeMode: AltitudeMode
  busy: boolean
  lastEvent: MissionEventPayload | null
  syncState: 'idle' | 'uploading' | 'downloading' | 'clearing'
  /** True when the plan was edited since it was last known to match the FC. */
  dirty: boolean
  /** Hash of the items at the last successful sync (upload or download). */
  lastSyncedHash: string | null
  /** Terrain drape result (S3/D5): dem version + report. */
  drape: DrapeState | null
  /** Last automatic (terrain-mode) drape failure, `null` when the last drape
   * succeeded. Without it a failed drape leaves stale heights silently. */
  drapeError: string | null
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
  /** Last inserted preset pattern (cleared by any waypoint edit). */
  lastPattern: InsertedPattern | null
  /** Pinned pattern centre (cloverleaf centre / rectangle centre), editable by
   * the panel inputs or by dragging the map handle (A2). */
  patternCenter: GeoPoint | null
  /** Waypoint the FC is flying now, from MISSION_CURRENT (0-based). */
  currentSeq: number | null
  select: (seq: number | null) => void
  setWaypoints: (waypoints: PlannedWaypoint[]) => void
  setAltitudeMode: (mode: AltitudeMode) => void
  /** Survey AGL in terrain mode (the Pattern altitude input), metres. */
  surveyAglM: number
  setSurveyAglM: (aglM: number) => void
  updatePosition: (seq: number, latDeg: number, lonDeg: number) => void
  updateAltitude: (seq: number, amslM: number) => void
  addWaypoint: (latDeg: number, lonDeg: number, amslM: number) => void
  /** Add a waypoint from a map click, inheriting the previous altitude. */
  addWaypointAt: (latDeg: number, lonDeg: number) => void
  removeWaypoint: (seq: number) => void
  moveWaypoint: (from: number, to: number) => void
  /** Append a generated pattern's waypoints and show its line table. */
  insertPattern: (
    plan: Pick<PatternPlan, 'waypoints' | 'lines'>,
    label: string,
    center?: GeoPoint | null,
    stats?: PatternStats | null,
  ) => void
  /** Drape the plan over the DEM of a dem-prep tile folder (S3/D5). */
  drapeToTerrain: (folder: string, aglM: number, minDsmClearanceM: number) => Promise<
    string | null
  >
  /** Pin or move the pattern centre (typed in the panel or dragged on the map). */
  setPatternCenter: (center: GeoPoint | null) => void
  upload: () => Promise<void>
  download: () => Promise<void>
  /** Clear the mission *on the flight controller* (link required). */
  clear: () => Promise<void>
  /** Discard the local plan (waypoints, complex blocks, home) — no link needed. */
  clearPlan: () => void
  /** The link dropped: no mission event will arrive, so stop showing progress. */
  linkLost: () => void
  setCurrent: (seq: number) => Promise<void>
  handleEvent: (e: MissionEventPayload) => void
  handlePlan: (items: MissionItem[]) => void
  /** Editable waypoints compiled to the current wire frame. */
  compiled: () => MissionItem[]
  /** Compiled waypoints plus complex-item children, in flyable order. */
  flyable: () => MissionItem[]
  applyImport: (result: PlanImport) => void
  verifyFc: () => Promise<void>
  reset: () => void
}

export { itemsHash } from '../mission/hash'

/** Home/ground AMSL anchor, from the imported plan's `plannedHomePosition`. */
function homeAmsl(home: [number, number, number] | null): number {
  return home?.[2] ?? 0
}

function useSelection(count: number, selectedSeq: number | null): number | null {
  if (selectedSeq === null) return null
  return selectedSeq >= 0 && selectedSeq < count ? selectedSeq : null
}

export const useMissionStore = create<MissionState>((set, get) => ({
  waypoints: [],
  selectedSeq: null,
  altitudeMode: 'relative',
  surveyAglM: 40,
  busy: false,
  lastEvent: null,
  syncState: 'idle',
  dirty: false,
  lastSyncedHash: null,
  fcMatches: null,
  verifying: false,
  drape: null,
  drapeError: null,
  planBase: null,
  blocks: [],
  home: null,
  lastPattern: null,
  /** Pinned pattern centre (cloverleaf / sweep rectangle), editable on the map. */
  patternCenter: null,
  currentSeq: null,

  select: (seq) => set({ selectedSeq: seq }),

  setWaypoints: (waypoints) =>
    set((s) => ({
      waypoints,
      selectedSeq: useSelection(waypoints.length, s.selectedSeq),
      dirty: true,
      fcMatches: null,
      lastPattern: null,
    })),

  setAltitudeMode: (mode) => set({ altitudeMode: mode }),
  setSurveyAglM: (aglM) => set({ surveyAglM: aglM }),

  updatePosition: (seq, latDeg, lonDeg) =>
    set((s) => ({
      waypoints: s.waypoints.map((wp, i) =>
        i === seq
          ? { ...wp, position: { latitude_deg: latDeg, longitude_deg: lonDeg } }
          : wp,
      ),
      dirty: true,
      fcMatches: null,
      lastPattern: null,
    })),

  updateAltitude: (seq, amslM) =>
    set((s) => ({
      waypoints: s.waypoints.map((wp, i) =>
        i === seq ? { ...wp, altitude: { datum: wp.altitude.datum, meters: amslM } } : wp,
      ),
      dirty: true,
      fcMatches: null,
      lastPattern: null,
    })),

  addWaypoint: (latDeg, lonDeg, amslM) =>
    set((s) => {
      const waypoints = [...s.waypoints, makeWaypoint(latDeg, lonDeg, amslM)]
      return {
        waypoints,
        selectedSeq: waypoints.length - 1,
        dirty: true,
        fcMatches: null,
        lastPattern: null,
      }
    }),

  addWaypointAt: (latDeg, lonDeg) =>
    set((s) => {
      // Inherit the last waypoint's altitude so a clicked line stays level
      // (WS-G G3); fall back to a default clearance above home.
      const last = s.waypoints[s.waypoints.length - 1]
      const amsl = last ? last.altitude.meters : (s.home?.[2] ?? 0) + DEFAULT_ALT_AGL_M
      const waypoints = [...s.waypoints, makeWaypoint(latDeg, lonDeg, amsl)]
      return {
        waypoints,
        selectedSeq: waypoints.length - 1,
        dirty: true,
        fcMatches: null,
        lastPattern: null,
      }
    }),

  removeWaypoint: (seq) =>
    set((s) => ({
      waypoints: s.waypoints.filter((_wp, i) => i !== seq),
      selectedSeq: null,
      dirty: true,
      fcMatches: null,
      lastPattern: null,
    })),

  moveWaypoint: (from, to) =>
    set((s) => {
      const waypoints = [...s.waypoints]
      if (from < 0 || from >= waypoints.length || to < 0 || to >= waypoints.length) return s
      const [moved] = waypoints.splice(from, 1)
      waypoints.splice(to, 0, moved)
      return {
        waypoints,
        selectedSeq: useSelection(waypoints.length, s.selectedSeq),
        dirty: true,
        fcMatches: null,
        lastPattern: null,
      }
    }),

  setPatternCenter: (center) =>
    set((s) => ({
      patternCenter: center,
      // The pattern block moves with its centre: a re-generate at a new
      // centre still trims the previous block correctly.
      lastPattern: s.lastPattern ? { ...s.lastPattern, center } : s.lastPattern,
    })),

  drapeToTerrain: async (folder, aglM, minDsmClearanceM) => {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const report = await invoke<DrapeReport>('terrain_drape', {
        waypoints: get().waypoints,
        folder,
        aglM,
        minDsmClearanceM,
      })
      set({
        waypoints: report.waypoints,
        dirty: true,
        fcMatches: null,
        drape: { report, folder },
      })
      return null
    } catch (e) {
      return String(e)
    }
  },

  insertPattern: (plan, label, center, stats) =>
    set((s) => {
      // Generate replaces the previous preset block rather than stacking a
      // second copy of the trajectory. `lastPattern` is cleared by every
      // manual edit, so when it is set those trailing waypoints are ours to
      // drop. Always trim to `min(previous, len)`: a re-generation with a
      // smaller pattern (or an external trim that did not clear lastPattern)
      // must never leave the old block stacked behind the new one
      // (operator report: cloverleaf re-generate kept the old trajectory).
      const previous = s.lastPattern?.count ?? 0
      const trimmed = previous > 0 ? Math.min(previous, s.waypoints.length) : 0
      const kept =
        trimmed > 0 ? s.waypoints.slice(0, s.waypoints.length - trimmed) : s.waypoints
      return {
        waypoints: [...kept, ...plan.waypoints],
        lastPattern: {
          label,
          count: plan.waypoints.length,
          lines: plan.lines.map((line) => ({
            ...line,
            start_seq: line.start_seq + kept.length,
            end_seq: line.end_seq + kept.length,
          })),
          center: center ?? null,
          stats: stats ?? null,
        },
        patternCenter: center ?? null,
        selectedSeq: null,
        dirty: true,
        fcMatches: null,
        drape: null,
      }
    }),

  clearPlan: () =>
    set({
      waypoints: [],
      blocks: [],
      planBase: null,
      home: null,
      lastPattern: null,
      patternCenter: null,
      selectedSeq: null,
      currentSeq: null,
      dirty: false,
      fcMatches: null,
      lastSyncedHash: null,
      lastEvent: null,
      verifying: false,
    }),

  // The mission service task exits with its connection (core::mission::service),
  // so a dead link never emits the terminal event. Without this the panel would
  // sit on "Uploading…" with every button disabled.
  linkLost: () =>
    set({ busy: false, verifying: false, syncState: 'idle' }),

  upload: async () => {
    const items = get().flyable()
    // Nothing to fly: the panel disables the button, but a clear-then-upload
    // race would otherwise leave `busy` set forever.
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
      const done = e.kind === 'completed' || e.kind === 'failed'
      const busy = done ? false : s.busy
      const syncState: MissionState['syncState'] = done
        ? 'idle'
        : e.op === 'upload' && e.kind === 'progress'
          ? 'uploading'
          : e.op === 'download' && e.kind === 'progress'
            ? 'downloading'
            : s.syncState
      const patch: Partial<MissionState> = { lastEvent: e, busy, syncState }
      if (e.kind === 'current_changed') patch.currentSeq = e.seq
      if (e.kind === 'failed') {
        // A failed operation ends any read-back too: without this a lost
        // `mission_plan` left `verifying` set and swallowed the next download.
        patch.verifying = false
      }
      if (e.kind === 'completed' && e.op === 'upload') {
        // We believe the FC now holds exactly our plan; confirm with a
        // read-back download (issues.md #15).
        patch.dirty = false
        patch.lastSyncedHash = itemsHash(
          orderedMissionItems(
            compileWaypoints(s.waypoints, s.altitudeMode, homeAmsl(s.home)),
            s.blocks,
            s.planBase ?? undefined,
          ),
        )
        patch.fcMatches = true
      }
      return patch
    })
    if (e.kind === 'completed' && e.op === 'upload') {
      void get().verifyFc()
    }
    // The FC is flying a mission this plan does not know about (typical:
    // the mission came from QGC or a previous session — the local plan is
    // memory-only and a restart empties it). Fetch it once so the Fly view
    // can show progress. Safe by construction: it only fires while the
    // local plan is empty, so nothing the operator built can be clobbered.
    if (
      e.kind === 'current_changed' &&
      get().waypoints.length === 0 &&
      get().blocks.length === 0 &&
      !get().busy &&
      !get().verifying
    ) {
      void get().download()
    }
  },

  handlePlan: (items) => {
    set((s) => {
      if (s.verifying) {
        return {
          verifying: false,
          fcMatches: itemsHash(items) === s.lastSyncedHash,
          busy: false,
          syncState: 'idle' as const,
        }
      }
      const anchor = homeAmsl(s.home)
      const mode = items.length > 0 ? modeFromFrame(items[0].frame) : null
      const current = items.findIndex((it) => it.current)
      return {
        currentSeq: current >= 0 ? current : null,
        waypoints: items.map((it) => waypointFromItem(it, anchor)),
        blocks: [],
        planBase: null,
        lastPattern: null,
        ...(mode ? { altitudeMode: mode } : {}),
        dirty: false,
        lastSyncedHash: itemsHash(items),
        fcMatches: true,
        busy: false,
        syncState: 'idle' as const,
        selectedSeq: null,
      }
    })
  },

  compiled: () => {
    const s = get()
    return compileWaypoints(s.waypoints, s.altitudeMode, homeAmsl(s.home))
  },

  flyable: () => {
    const s = get()
    return orderedMissionItems(
      compileWaypoints(s.waypoints, s.altitudeMode, homeAmsl(s.home)),
      s.blocks,
      s.planBase ?? undefined,
    )
  },

  applyImport: (result) => {
    const anchor = homeAmsl(result.home)
    set({
      waypoints: result.items.map((it) => waypointFromItem(it, anchor)),
      altitudeMode: result.mode,
      selectedSeq: null,
      lastPattern: null,
      dirty: true,
      fcMatches: null,
      planBase: result.base,
      blocks: result.blocks,
      home: result.home,
    })
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
      waypoints: [],
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
      lastPattern: null,
      patternCenter: null,
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

// ---------------------------------------------------------------------------
// Terrain mode: automatic re-drape (ADR-005/013 addendum, plan C).
//
// In `terrain` mode the stored waypoints are absolute AMSL but their *input*
// is the survey AGL: every plan change (pattern generate, waypoint drag,
// AGL change, DEM change) is re-draped over the loaded DEM so the flight
// keeps hugging the terrain. The drape output is compared against the last
// draped input — identical plan+AGL+DEM short-circuits the loop the drape's
// own `set` would otherwise create.
// ---------------------------------------------------------------------------

/** Milliseconds to settle plan edits before re-draping. */
const TERRAIN_DRAPE_DEBOUNCE_MS = 600

let drapeTimer: ReturnType<typeof setTimeout> | undefined
/** Input fingerprint of the last completed drape (plan + AGL + DEM). */
let lastDrapedKey = ''
let draping = false

function planKey(waypoints: PlannedWaypoint[]): string {
  return JSON.stringify(
    waypoints.map((w) => [
      w.command,
      w.position.latitude_deg,
      w.position.longitude_deg,
      Math.fround(w.altitude.meters),
    ]),
  )
}

function scheduleTerrainDrape(): void {
  if (drapeTimer !== undefined) clearTimeout(drapeTimer)
  drapeTimer = setTimeout(() => void runTerrainDrape(), TERRAIN_DRAPE_DEBOUNCE_MS)
}

async function runTerrainDrape(): Promise<void> {
  const s = useMissionStore.getState()
  if (s.altitudeMode !== 'terrain' || s.waypoints.length === 0 || draping) return
  const terrain = useTerrainStore.getState()
  const tile = terrain.tile
  if (!tile) return // no DEM: plain heights stay, the panel says so
  const key = `${planKey(s.waypoints)}|${s.surveyAglM}|${tile.version}`
  if (key === lastDrapedKey) return // already draped against this input
  draping = true
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const report = await invoke<DrapeReport>('terrain_drape', {
      waypoints: s.waypoints,
      folder: tile.path,
      aglM: s.surveyAglM,
      minDsmClearanceM: 5,
    })
    // The plan may have been edited while the backend was draping: applying
    // the result would overwrite those edits with a stale snapshot. Drop it
    // and re-drape the current plan instead.
    if (planKey(useMissionStore.getState().waypoints) !== planKey(s.waypoints)) {
      scheduleTerrainDrape()
      return
    }
    lastDrapedKey = `${planKey(report.waypoints)}|${s.surveyAglM}|${tile.version}`
    useMissionStore.setState({
      waypoints: report.waypoints,
      dirty: true,
      fcMatches: null,
      drape: { report, folder: tile.path },
      drapeError: null,
    })
  } catch (e) {
    // Outside coverage / DEM void: keep the plan editable but say so — the
    // heights are NOT terrain-following until this clears.
    useMissionStore.setState({ drapeError: String(e) })
  } finally {
    draping = false
  }
}

// Re-drape whenever the plan, the AGL or the loaded DEM changes while in
// terrain mode. (subscribe fires on every store set; the key check keeps it
// from re-running on its own output.)
useMissionStore.subscribe((s) => {
  if (s.altitudeMode === 'terrain') scheduleTerrainDrape()
})
