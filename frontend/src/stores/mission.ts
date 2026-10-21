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
import { applyCompiled } from '../mission/drapeApply'
import { drapeErrorReason } from '../util/drapeError'
import type { DrapePreview, DrapeLineInput } from '../mission/drapePreview'
import { useTerrainStore } from './terrain'
import { DRAPE_ALERT_KEY, clearAlert, raiseAlert } from './alerts'

/**
 * Altitude semantics of the editable plan.
 * - `relative`  — z stored as AMSL, flown as HOME-relative (compile converts).
 * - `amsl`      — z stored and flown as AMSL.
 * - `agl`       — z stored as AMSL; the HUD/labels show height − HOME AMSL.
 * - `terrain`   — terrain following (ADR-005/013 addendum): the Pattern
 *   altitude is a **survey AGL**; every plan change is automatically draped
 *   over the loaded surface — the live 3D mesh first (the surface on screen),
 *   else the 2D DEM tile (DTM + AGL, raised above the DSM clearance) — so the
 *   stored waypoints stay absolute AMSL (ADR-013 discipline) while always
 *   hugging the terrain. Without a surface the plan keeps plain heights, the
 *   panel warns and the waypoint rows mark their AGL unknown.
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

/** Result of the A9 drape (the solver pipeline) kept for the readouts. */
export interface DrapeState {
  /** The dem-prep tile folder the drape was computed against. */
  folder: string
  /** The DEM's provenance hash (dem_ref → archives). */
  demVersion: string
  /** Tightest planned DSM clearance in the result, `null` without a DSM. */
  minClearanceM: number | null
  /** Compiled route items now in the plan (speed items included). */
  compiledCount: number
  /** Landing items placed on the DEM surface. */
  landed: number
  /** Pinned heights that survived the re-drape. */
  pinned: number
  /** The DEM carries a DTM (false = DTM-only: AGL is height above surface). */
  hasDtm: boolean
}

/**
 * The plan-editing slice captured for undo/redo (U8). Terrain-drape output and
 * sync bookkeeping are deliberately excluded: an undo returns the *edit* that
 * preceded the current one and lets the terrain mode re-drape the restored
 * plan, rather than replaying a stale drape.
 */
export interface PlanSnapshot {
  waypoints: PlannedWaypoint[]
  blocks: PlanBlock[]
  home: [number, number, number] | null
  planBase: QgcPlan | null
  lastPattern: InsertedPattern | null
  patternCenter: GeoPoint | null
  selectedSeq: number | null
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
  /** Terrain drape result (A9): the solver pipeline's facts for the readout. */
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
  /** True right after the user cleared the local plan: the FC's mission must
   * not be auto-downloaded back over an intentionally empty plan (a running
   * mission would otherwise repopulate the panel via `current_changed`).
   * Cleared whenever a plan is explicitly set, added to, imported or
   * downloaded again. */
  planCleared: boolean
  /** Undo/redo stacks over the plan-editing slice (U8). */
  undoStack: PlanSnapshot[]
  redoStack: PlanSnapshot[]
  select: (seq: number | null) => void
  setWaypoints: (waypoints: PlannedWaypoint[]) => void
  setAltitudeMode: (mode: AltitudeMode) => void
  /** Survey AGL in terrain mode (the Pattern altitude input), metres. */
  surveyAglM: number
  /** Drape parameters (DR1, ADR-022): `minClearanceM` is REQUIRED — the drape
   * refuses to run without it (no silent default). */
  drapeSpec: DrapeSpecUi
  setDrapeSpec: (patch: Partial<DrapeSpecUi>) => void
  /** The DR4 solver preview over the last pattern (terrain mode), feeding the
   * profile chart and the Drape checklist facts. */
  drapePreview: DrapePreview | null
  runDrapePreview: () => Promise<void>
  /** U5/DR4: the `spec_hash` whose out-of-band spans the operator has seen
   * and accepted (the checklist then passes `infeasible_confirmed`). Any new
   * solve (different hash) invalidates it by comparison. */
  infeasibleAcceptedFor: string | null
  acceptInfeasible: (specHash: string) => void
  setSurveyAglM: (aglM: number) => void
  updatePosition: (seq: number, latDeg: number, lonDeg: number) => void
  updateAltitude: (seq: number, amslM: number) => void
  /** Pin/unpin a waypoint's height: terrain re-drape skips locked points. */
  toggleWaypointLock: (seq: number) => void
  addWaypoint: (latDeg: number, lonDeg: number, amslM: number) => void
  /** Add a waypoint from a map click, inheriting the previous altitude. */
  addWaypointAt: (latDeg: number, lonDeg: number) => void
  removeWaypoint: (seq: number) => void
  /** U17c: remove several waypoints as a single undoable edit. */
  removeWaypoints: (seqs: number[]) => void
  /** U17c: set the AMSL altitude of several waypoints as one undoable edit. */
  setAltitudeMany: (seqs: number[], amslM: number) => void
  moveWaypoint: (from: number, to: number) => void
  /** Append a generated pattern's waypoints and show its line table. */
  insertPattern: (
    plan: Pick<PatternPlan, 'waypoints' | 'lines'>,
    label: string,
    center?: GeoPoint | null,
    stats?: PatternStats | null,
  ) => void
  /** Drape the plan with the one solver pipeline (A9): solve + compile over
   * the pattern lines, apply the compiled route, land on the DEM surface. */
  drapeToTerrain: () => Promise<string | null>
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
  /** The heartbeat came back: re-read the mission state the FC holds (L3). */
  linkRestored: () => void
  setCurrent: (seq: number) => Promise<void>
  handleEvent: (e: MissionEventPayload) => void
  handlePlan: (items: MissionItem[]) => void
  /** Editable waypoints compiled to the current wire frame. */
  compiled: () => MissionItem[]
  /** Compiled waypoints plus complex-item children, in flyable order. */
  flyable: () => MissionItem[]
  applyImport: (result: PlanImport) => void
  verifyFc: () => Promise<void>
  /** Revert the last plan edit (U8). No-op on an empty stack. */
  undo: () => void
  /** Re-apply the last undone plan edit (U8). No-op on an empty stack. */
  redo: () => void
  reset: () => void
}

export { itemsHash } from '../mission/hash'

/** Home/ground AMSL anchor, from the imported plan's `plannedHomePosition`. */
function homeAmsl(home: [number, number, number] | null): number {
  return home?.[2] ?? 0
}

/** Cap the undo depth so a long editing session cannot grow unbounded. */
const HISTORY_LIMIT = 100

/** The plan-editing slice of the current state. */
function snapshot(s: MissionState): PlanSnapshot {
  return {
    waypoints: s.waypoints,
    blocks: s.blocks,
    home: s.home,
    planBase: s.planBase,
    lastPattern: s.lastPattern,
    patternCenter: s.patternCenter,
    selectedSeq: s.selectedSeq,
  }
}

/** Push the current plan onto the undo stack and drop the redo branch. Every
 * mutating action spreads this into its patch so undo always has the *prior*
 * state to return to. */
function withHistory(s: MissionState): Pick<MissionState, 'undoStack' | 'redoStack'> {
  const undoStack = [...s.undoStack, snapshot(s)]
  if (undoStack.length > HISTORY_LIMIT) undoStack.shift()
  return { undoStack, redoStack: [] }
}

function useSelection(count: number, selectedSeq: number | null): number | null {
  if (selectedSeq === null) return null
  return selectedSeq >= 0 && selectedSeq < count ? selectedSeq : null
}

/** Drape parameters the operator enters (DR1). Defaults mirror
 * `core::terrain::drape_spec::DrapeSpec::default`, except the clearance which
 * has NO default by rule (§5B principle 5). */
export interface DrapeSpecUi {
  minClearanceM: number | null
  verticalOffsetM: number
  verticalOffsetNote: string
  posMarginM: number
  dsmFootprintM: number
  maxClimbMps: number
  maxDescentMps: number
  sampleStepM: number
  runinM: number
  wpTolM: number
}

const DEFAULT_DRAPE_SPEC: DrapeSpecUi = {
  minClearanceM: null,
  verticalOffsetM: 0,
  verticalOffsetNote: '',
  posMarginM: 1,
  dsmFootprintM: 1,
  maxClimbMps: 3,
  maxDescentMps: 3,
  sampleStepM: 1,
  runinM: 10,
  wpTolM: 1,
}


/** Drape failure reasons the panel renders after its own label. */
/** H7 fail-closed: a CGVD2013 source without the full CGG2013/EGM96 chain
 * must not produce AMSL heights — and the AMSL export is blocked while this
 * is the latest drape error. */
export const VERTICAL_DATUM_UNVERIFIED_ERROR =
  'vertical datum unverified — the CGG2013/EGM96 height chain is unavailable under a draped waypoint'
export const useMissionStore = create<MissionState>((set, get) => ({
  waypoints: [],
  selectedSeq: null,
  altitudeMode: 'relative',
  surveyAglM: 40,
  drapeSpec: DEFAULT_DRAPE_SPEC,
  drapePreview: null,
  infeasibleAcceptedFor: null,
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
  planCleared: false,
  undoStack: [],
  redoStack: [],

  select: (seq) => set({ selectedSeq: seq }),

  setWaypoints: (waypoints) =>
    set((s) => ({
      ...withHistory(s),
      waypoints,
      selectedSeq: useSelection(waypoints.length, s.selectedSeq),
      dirty: true,
      fcMatches: null,
      lastPattern: null,
      planCleared: false,
    })),

  // A mode switch does not change the waypoints, but it changes what the plan
  // means, so it is undoable too. The survey AGL is a live input (every
  // keystroke re-drapes) and is deliberately *not* pushed to history.
  setAltitudeMode: (mode) => {
    set((s) => ({ ...withHistory(s), altitudeMode: mode }))
    if (mode === 'terrain') {
      // Entering terrain mode: resolve a surface automatically (no dialog) —
      // a remembered dem-prep folder, the prepared survey-area HRDEM session,
      // or a DEM beside the app — then re-drape. Loading the tile does not
      // fire this store's subscription, so nudge the drape once it resolves.
      void (async () => {
        const { useTerrainStore } = await import('./terrain')
        const path = await useTerrainStore.getState().ensureTile(false)
        if (path) scheduleTerrainDrape()
      })()
    }
  },
  setSurveyAglM: (aglM) => set({ surveyAglM: aglM }),
  setDrapeSpec: (patch) => set((st) => ({ drapeSpec: { ...st.drapeSpec, ...patch } })),

  runDrapePreview: async () => {
    await runDrapeOnce(false)
  },

  acceptInfeasible: (specHash) => set({ infeasibleAcceptedFor: specHash }),

  updatePosition: (seq, latDeg, lonDeg) =>
    set((s) => ({
      ...withHistory(s),
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
      ...withHistory(s),
      waypoints: s.waypoints.map((wp, i) =>
        i === seq ? { ...wp, altitude: { datum: wp.altitude.datum, meters: amslM } } : wp,
      ),
      dirty: true,
      fcMatches: null,
      lastPattern: null,
    })),

  toggleWaypointLock: (seq) =>
    set((s) => ({
      ...withHistory(s),
      waypoints: s.waypoints.map((wp, i) => (i === seq ? { ...wp, locked: !wp.locked } : wp)),
      dirty: true,
      fcMatches: null,
    })),

  addWaypoint: (latDeg, lonDeg, amslM) =>
    set((s) => {
      const waypoints = [...s.waypoints, makeWaypoint(latDeg, lonDeg, amslM)]
      return {
        ...withHistory(s),
        waypoints,
        selectedSeq: waypoints.length - 1,
        dirty: true,
        fcMatches: null,
        lastPattern: null,
        planCleared: false,
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
        ...withHistory(s),
        waypoints,
        selectedSeq: waypoints.length - 1,
        dirty: true,
        fcMatches: null,
        lastPattern: null,
        planCleared: false,
      }
    }),

  removeWaypoint: (seq) =>
    set((s) => ({
      ...withHistory(s),
      waypoints: s.waypoints.filter((_wp, i) => i !== seq),
      selectedSeq: null,
      dirty: true,
      fcMatches: null,
      lastPattern: null,
    })),

  // U17c: batch edits are one history entry, so Ctrl+Z restores the whole
  // selection at once instead of peeling the rows off one at a time.
  removeWaypoints: (seqs) =>
    set((s) => {
      if (seqs.length === 0) return {}
      const drop = new Set(seqs)
      const waypoints = s.waypoints.filter((_wp, i) => !drop.has(i))
      return {
        ...withHistory(s),
        waypoints,
        selectedSeq: useSelection(waypoints.length, s.selectedSeq),
        dirty: true,
        fcMatches: null,
        lastPattern: null,
      }
    }),

  setAltitudeMany: (seqs, amslM) =>
    set((s) => {
      if (seqs.length === 0) return {}
      const target = new Set(seqs)
      return {
        ...withHistory(s),
        waypoints: s.waypoints.map((wp, i) =>
          target.has(i) ? { ...wp, altitude: { datum: wp.altitude.datum, meters: amslM } } : wp,
        ),
        dirty: true,
        fcMatches: null,
        lastPattern: null,
      }
    }),

  moveWaypoint: (from, to) =>
    set((s) => {
      const waypoints = [...s.waypoints]
      if (from < 0 || from >= waypoints.length || to < 0 || to >= waypoints.length) return s
      const [moved] = waypoints.splice(from, 1)
      waypoints.splice(to, 0, moved)
      return {
        ...withHistory(s),
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

  drapeToTerrain: async () => {
    const err = await runDrapeOnce(true)
    return err
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
        ...withHistory(s),
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
        drapeError: null,
        infeasibleAcceptedFor: null,
        planCleared: false,
      }
    }),

  clearPlan: () =>
    set((s) => ({
      ...withHistory(s),
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
      // An intentional empty plan: the FC's mission must not be fetched back
      // over it (a running mission would repopulate the panel).
      planCleared: true,
    })),

  // The mission service task exits with its connection (core::mission::service),
  // so a dead link never emits the terminal event. Without this the panel would
  // sit on "Uploading…" with every button disabled.
  linkLost: () =>
    set({ busy: false, verifying: false, syncState: 'idle' }),

  // Reconnect (L3, plan §4): mode and armed state ride on the heartbeat and
  // resume by themselves, but the *mission* state is only known from an
  // exchange. Re-read the sync state in whichever way is already meaningful:
  // a plan we synced gets its read-back; an empty panel fetches whatever the
  // FC is flying (the same rule `current_changed` uses). An in-flight
  // operation is never interrupted.
  linkRestored: () => {
    const s = get()
    if (s.busy || s.verifying) return
    if (s.lastSyncedHash) void get().verifyFc()
    else if (s.waypoints.length === 0 && s.blocks.length === 0 && !s.planCleared) {
      void get().download()
    }
  },

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
      if (e.kind === 'completed' && e.op === 'clear') {
        // The FC dropped its mission: there is no baseline any more. The
        // local plan (if any) is simply unsynced again — reporting "synced"
        // here is exactly the stale-state false positive issues.md #33/#38
        // describe (the panel claimed sync while the FC held nothing).
        patch.lastSyncedHash = null
        patch.fcMatches = null
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
      !get().verifying &&
      // After an explicit "Clear local plan" the panel must stay empty: the
      // FC's still-loaded mission is not fetched back over an intentional
      // clear (operator report).
      !get().planCleared
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
        planCleared: false,
        // A plan fetched from the FC is a new baseline, not an edit: it resets
        // history so undo cannot resurrect a pre-download plan (U8).
        undoStack: [],
        redoStack: [],
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
    set((s) => ({
      ...withHistory(s),
      waypoints: result.items.map((it) => waypointFromItem(it, anchor)),
      altitudeMode: result.mode,
      selectedSeq: null,
      lastPattern: null,
      dirty: true,
      fcMatches: null,
      planBase: result.base,
      blocks: result.blocks,
      home: result.home,
      planCleared: false,
    }))
  },

  // Undo/redo swap the plan slice between the two stacks. The terrain-mode
  // subscription re-drapes the restored plan on its own; sync bookkeeping
  // (`lastSyncedHash`) survives so the panel still knows the FC baseline.
  undo: () =>
    set((s) => {
      const previous = s.undoStack[s.undoStack.length - 1]
      if (!previous) return s
      return {
        ...previous,
        undoStack: s.undoStack.slice(0, -1),
        redoStack: [...s.redoStack, snapshot(s)],
        dirty: true,
        fcMatches: null,
      }
    }),

  redo: () =>
    set((s) => {
      const next = s.redoStack[s.redoStack.length - 1]
      if (!next) return s
      return {
        ...next,
        undoStack: [...s.undoStack, snapshot(s)],
        redoStack: s.redoStack.slice(0, -1),
        dirty: true,
        fcMatches: null,
      }
    }),

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
      drape: null,
      drapeError: null,
      infeasibleAcceptedFor: null,
      planBase: null,
      blocks: [],
      home: null,
      lastPattern: null,
      patternCenter: null,
      currentSeq: null,
      undoStack: [],
      redoStack: [],
      // A true fresh boot: the FC's running mission may be auto-fetched.
      planCleared: false,
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
// Terrain mode: automatic re-drape (ADR-005/013 addendum, A9).
//
// In `terrain` mode the stored waypoints are absolute AMSL but their *input*
// is the survey AGL: every plan change (pattern generate, AGL change, DEM
// change) re-runs the one drape pipeline — `drape_preview` (solve → compile,
// ADR-022) — and applies the compiled route to the plan. The interim
// per-waypoint bridge (`terrain_drape` command + `mission/meshDrape.ts`) was
// removed: the grid/mesh is display-only, the DEM tile is the only drape
// reference (fail-closed when no DEM covers the plan).
//
// The drape output is compared against the last draped input — identical
// plan+AGL+DEM short-circuits the loop the drape's own `set` would otherwise
// create.
// ---------------------------------------------------------------------------

/** Milliseconds to settle plan edits before re-draping. */
const TERRAIN_DRAPE_DEBOUNCE_MS = 600

let drapeTimer: ReturnType<typeof setTimeout> | undefined
/** Input fingerprint of the last completed drape (plan + AGL + surface). */
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

/** DR1: `min_clearance_dsm` is required — the drape refuses to run without
 * it, with the reason the panel shows (§5B principle 5: no silent default). */
const DEM_CLEARANCE_REQUIRED_ERROR =
  'enter the minimum DSM clearance in the drape parameters before generating'

/** Manual edits leave the drape nothing to solve: the pattern lines are the
 * solver's input, so a hand-moved waypoint must be regenerated, not draped. */
const PATTERN_REQUIRED_ERROR =
  'the pattern was edited by hand — regenerate it so the drape has lines to solve'

/** The pattern block the generated lines occupy, or `null` without one. */
function patternBlock(
  pattern: NonNullable<ReturnType<typeof useMissionStore.getState>['lastPattern']>,
): { start: number; end: number } | null {
  if (pattern.lines.length === 0) return null
  const start = Math.min(...pattern.lines.map((l) => l.start_seq))
  const end = Math.max(...pattern.lines.map((l) => l.end_seq))
  return { start, end }
}

/**
 * The one drape pipeline (A9): solve + compile over the pattern lines, then
 * — with `apply` — write the compiled route into the plan and refresh the
 * preview facts. Preview-only runs never touch the waypoints.
 *
 * # Errors
 *
 * Returns the failure reason (already folded for the panel) when the solver
 * or compiler refuses, a landing point is not sampleable, or the vertical
 * datum chain is unavailable — the plan keeps its heights either way.
 */
async function runDrapeOnce(apply: boolean): Promise<string | null> {
  const s = useMissionStore.getState()
  const terrain = useTerrainStore.getState()
  const folder = terrain.tile?.path
  const spec = s.drapeSpec
  const pattern = s.lastPattern
  if (!pattern || !folder || !terrain.tile) {
    if (!apply) setPreview(null)
    return apply ? PATTERN_REQUIRED_ERROR : null
  }
  const block = patternBlock(pattern)
  if (!block) {
    if (!apply) setPreview(null)
    return apply ? PATTERN_REQUIRED_ERROR : null
  }
  const clearance = spec.minClearanceM
  if (clearance == null || !Number.isFinite(clearance) || clearance < 0) {
    if (!apply) setPreview(null)
    return apply ? DEM_CLEARANCE_REQUIRED_ERROR : null
  }

  // One input line per pattern line: its waypoints in mission order.
  const bySeq = new Map(s.waypoints.map((w, i) => [i, w]))
  const lines = pattern.lines
    .map((l, i) => {
      const pts: Array<[number, number]> = []
      for (let seq = l.start_seq; seq <= l.end_seq; seq += 1) {
        const w = bySeq.get(seq)
        if (w) pts.push([w.position.longitude_deg, w.position.latitude_deg])
      }
      return pts.length >= 2
        ? {
            id: `${l.kind}-${i + 1}`,
            kind: l.kind as string,
            vertices: pts,
          }
        : null
    })
    .filter((l): l is DrapeLineInput => l !== null)
  if (lines.length === 0) {
    if (!apply) setPreview(null)
    return apply ? PATTERN_REQUIRED_ERROR : null
  }

  // Landing items (anywhere in the plan) are placed on the DEM surface by
  // the same command: neither the survey AGL nor the clearance applies.
  const landing: Array<[number, number]> = s.waypoints
    .filter((w) => (w.command === 21 || w.command === 85) && !w.locked)
    .map((w) => [w.position.longitude_deg, w.position.latitude_deg])

  let preview: DrapePreview
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    preview = await invoke<DrapePreview>('drape_preview', {
      folder,
      spec: {
        version: 1,
        target_agl_m: s.surveyAglM,
        agl_band_m: 3,
        min_clearance_dsm_m: clearance,
        pos_margin_m: spec.posMarginM,
        dsm_footprint_m: spec.dsmFootprintM,
        max_climb_rate_mps: spec.maxClimbMps,
        max_descent_rate_mps: spec.maxDescentMps,
        ground_speed_mps: 5,
        sample_step_m: spec.sampleStepM,
        smoothing_m: 0,
        runin_length_m: spec.runinM,
        turn_policy: 'standard',
        wp_tol_m: spec.wpTolM,
        vertical_offset: {
          meters: spec.verticalOffsetM,
          origin: 'operator',
          note: spec.verticalOffsetNote,
        },
      },
      lines,
      landing,
    })
  } catch (e) {
    // Solver refusal / datum chain unavailable: the preview is stale, and a
    // failed drape must not leave stale heights looking draped.
    if (!apply) setPreview(null)
    return drapeErrorReason(String(e))
  }
  setPreview(preview)
  if (!apply) return null

  // The plan may have been edited while the backend ran: applying the
  // result would overwrite those edits with a stale snapshot. Drop it —
  // the edit's own subscription already scheduled a re-drape.
  if (planKey(useMissionStore.getState().waypoints) !== planKey(s.waypoints)) return null

  const landingHeights = new Map<string, number>()
  landing.forEach((pt, i) => {
    const h = preview.landing_amsl_m[i]
    if (h != null && Number.isFinite(h)) {
      landingHeights.set(`${pt[1].toFixed(9)},${pt[0].toFixed(9)}`, h)
    }
  })
  const applied = applyCompiled(s.waypoints, block, preview.result.compiled, landingHeights)
  const drape: DrapeState = {
    folder,
    demVersion: preview.result.dem_ref.hash || terrain.tile.version,
    minClearanceM: preview.result.stats.min_clearance_m,
    compiledCount: preview.waypoints.length,
    landed: applied.landed,
    pinned: applied.pinned,
    hasDtm: preview.has_dtm,
  }
  // The pattern block moved (speed items joined the route): re-anchor the
  // line table so the map colours and the next preview read the right seqs.
  const linesByKey = new Map(applied.lines.map((l) => [l.kind, l]))
  const reanchored = pattern.lines.map((l) => {
    const next = linesByKey.get(l.kind)
    return next ? { ...l, start_seq: next.start_seq, end_seq: next.end_seq } : l
  })
  // A re-drape that produced byte-identical heights (mode toggle, surface
  // reload, the same inputs) must not re-mark the plan dirty — otherwise
  // the Sync step flips to "Unsaved changes" right after a successful
  // upload for no operator-visible reason.
  const unchanged = planKey(applied.waypoints) === planKey(s.waypoints)
  useMissionStore.setState({
    waypoints: applied.waypoints,
    ...(unchanged ? {} : { dirty: true, fcMatches: null }),
    drape,
    drapeError: null,
    lastPattern: {
      label: pattern.label,
      count: applied.blockEnd - applied.blockStart + 1,
      lines: reanchored,
      center: pattern.center,
      stats: pattern.stats,
    },
  })
  return null

  function setPreview(p: DrapePreview | null): void {
    useMissionStore.setState({ drapePreview: p })
  }
}

function scheduleTerrainDrape(): void {
  if (drapeTimer !== undefined) clearTimeout(drapeTimer)
  drapeTimer = setTimeout(() => {
    void runTerrainDrape()
  }, TERRAIN_DRAPE_DEBOUNCE_MS)
}

async function runTerrainDrape(): Promise<void> {
  const s = useMissionStore.getState()
  if (s.altitudeMode !== 'terrain' || s.waypoints.length === 0 || draping) return
  if (!useTerrainStore.getState().tile) return // no DEM: heights stay, the panel says so
  // The required-value gate comes before anything else: no clearance, no drape.
  const clearance = s.drapeSpec.minClearanceM
  if (clearance == null || !Number.isFinite(clearance) || clearance < 0) {
    if (lastDrapedKey !== DEM_CLEARANCE_REQUIRED_ERROR) {
      lastDrapedKey = DEM_CLEARANCE_REQUIRED_ERROR
      useMissionStore.setState({ drape: null, drapeError: DEM_CLEARANCE_REQUIRED_ERROR })
      raiseAlert({
        source: 'dem',
        severity: 'warn',
        text: `Terrain drape failed: ${DEM_CLEARANCE_REQUIRED_ERROR}`,
        key: DRAPE_ALERT_KEY,
      })
    }
    return
  }
  const tileVersion = useTerrainStore.getState().tile?.version ?? ''
  const key = `${planKey(s.waypoints)}|${s.surveyAglM}|dem:${tileVersion}|${s.drapeSpec.verticalOffsetM}|${clearance}`
  if (key === lastDrapedKey) return // already draped against this input
  draping = true
  try {
    const outcome = await runDrapeOnce(true)
    if (outcome != null) {
      // Remember the failed input too: the panel's error line is a store
      // write, and re-running would retry the same failing input forever.
      lastDrapedKey = key
      useMissionStore.setState({ drape: null, drapeError: outcome })
      // One alert row for the whole condition (A3): the plan view shows the
      // detail, the badge shows that something is wrong from any view.
      raiseAlert({
        source: 'dem',
        severity: 'warn',
        text: `Terrain drape failed: ${outcome}`,
        key: DRAPE_ALERT_KEY,
      })
      return
    }
    const after = useMissionStore.getState()
    lastDrapedKey = `${planKey(after.waypoints)}|${s.surveyAglM}|dem:${tileVersion}`
    if (s.drapeError) {
      // Recovered: the terrain condition is gone, so retire its alert row.
      clearAlert('dem', DRAPE_ALERT_KEY)
    }
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

// Dev-only handle for inspecting the app from the DevTools protocol / E2E
// scripts (same pattern as `window.__mgViewer` in `useCesiumViewer`).
if (import.meta.env?.DEV && typeof window !== 'undefined') {
  ;(window as unknown as { __mgMission?: unknown }).__mgMission = useMissionStore
}
