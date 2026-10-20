// The drape monitor store (DR6): runs the DR5 engine in the backend over
// the live (anchored) telemetry while the Fly view is up and a compiled
// drape result exists, maps its edge-triggered alerts onto the alert center
// (A3), and exposes the MonitorState the Drape panel renders.
//
// The monitor runs only while: the vehicle link is up, a mission is
// uploaded+verified, and the drape preview (solve+compile) succeeded —
// matching the DR5 facts gating in `buildPlanState` (operator decision
// 2026-10-18: a manual flight is never judged against a route the FC does
// not fly).

import { invoke } from '@tauri-apps/api/core'
import { create } from 'zustand'
import type { MonitorState } from '../generated-types/MonitorState'
import type { MonitorAlert } from '../generated-types/MonitorAlert'
import { isTauri } from '../inspector/mock'
import { raiseAlert } from './alerts'
import { useMissionStore } from './mission'
import { useTelemetryStore } from './telemetry'
import * as terrainModule from './terrain'

/** Step cadence: the metrics do not change faster than the eye reads them. */
const STEP_MS = 500

interface DrapeMonitorState {
  running: boolean
  state: MonitorState | null
  /** Ensure the backend session matches the current (verified) plan. */
  ensure: () => Promise<void>
  stop: () => Promise<void>
  ack: () => Promise<void>
}

export const useDrapeMonitorStore = create<DrapeMonitorState>((set, get) => ({
  running: false,
  state: null,

  ensure: async () => {
    if (!isTauri()) return
    const m = useMissionStore.getState()
    // Only a verified upload carries the route the FC actually flies.
    const plan = { hasPlan: m.waypoints.length > 0 }
    const inSync = !m.dirty && !m.planCleared && m.syncState === 'idle'
    const verified = m.fcMatches === true && inSync && plan.hasPlan
    const preview = m.drapePreview
    if (!verified || !preview || preview.result.compiled.length === 0) {
      if (get().running) await get().stop()
      return
    }
    if (get().running) return
    try {
      const spec = await drapeSpecForMonitor()
      await invoke('drape_monitor_start', {
        folder: previewFolder(),
        result: preview.result,
        spec,
      })
      set({ running: true })
    } catch {
      set({ running: false })
    }
  },

  stop: async () => {
    set({ running: false, state: null })
    if (!isTauri()) return
    try {
      await invoke('drape_monitor_stop')
    } catch {
      /* nothing was running */
    }
  },

  ack: async () => {
    if (!isTauri()) return
    try {
      await invoke('drape_monitor_ack')
    } catch {
      /* nothing was running */
    }
  },
}))

/** The DEM folder the preview was solved against (the auto-picked tile). */
function previewFolder(): string {
  // The preview ran `drape_preview` against the terrain store's auto-picked
  // tile; the monitor must sample the very same DEM.
  const terrain = (useTerrainStoreRef() as unknown as { getState: () => { tile: { path: string } | null } })
  return terrain.getState().tile?.path ?? ''
}

/** Late terrain import (mission ↔ terrain already import each other). */
function useTerrainStoreRef() {
  return terrainModule.useTerrainStore
}

/** Rebuild the wire spec from the operator's drape parameters (DR1 UI). */
async function drapeSpecForMonitor() {
  const spec = useMissionStore.getState().drapeSpec
  const m = useMissionStore.getState()
  return {
    version: 1,
    target_agl_m: m.surveyAglM,
    agl_band_m: 3,
    min_clearance_dsm_m: spec.minClearanceM,
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
  }
}

/** Map the engine's edge-triggered alerts onto the alert center (A3). */
function raiseMonitorAlerts(alerts: MonitorAlert[]): void {
  for (const a of alerts) {
    raiseAlert({
      source: 'dem',
      severity: a.severity,
      text: a.text,
      key: `drape-${a.code}${a.line_id != null ? `-${a.line_id}` : ''}`,
    })
  }
}

// The step loop: one subscription, throttled, guarded by `running`.
if (isTauri()) {
  let lastStep = 0
  let stepping = false
  useTelemetryStore.subscribe((s) => {
    if (!useDrapeMonitorStore.getState().running || !s.snapshot) return
    const now = performance.now()
    if (now - lastStep < STEP_MS || stepping) return
    lastStep = now
    const snap = s.snapshot
    const pos = snap.global_position
    if (!pos) return
    void snap
    // Staleness is judged on the wall clock the fix carries.
    const atMs = Date.now()
    stepping = true
    void (async () => {
      try {
        const anchored = await anchoredAmsl(pos)
        const state = await invoke<MonitorState>('drape_monitor_step', {
          fix: {
            lat: pos.latitude_deg,
            lon: pos.longitude_deg,
            zAmslM: anchored,
            vnMps: pos.velocity.x_m_s,
            veMps: pos.velocity.y_m_s,
            vdMps: pos.velocity.z_m_s,
            atMs,
          },
          radarM: distanceSensorAgl(snap),
        })
        useDrapeMonitorStore.setState({ state })
        raiseMonitorAlerts(state.alerts ?? [])
      } catch {
        // Session gone or not startable: keep the last state visible.
      } finally {
        stepping = false
      }
    })()
  })
}

/** The DEM-frame AMSL: the same anchoring the map renders. */
async function anchoredAmsl(pos: { altitude: { meters: number }; relative_alt_m: number }): Promise<number> {
  const { vehicleAmslAnchoredM } = await import('../cesium/heights')
  const terrain = await import('./terrain')
  return vehicleAmslAnchoredM({
    anchorOffsetM: terrain.useTerrainStore.getState().amslAnchorOffsetM,
    demGroundAtHomeM: terrain.useTerrainStore.getState().homeGroundAmsl,
    reportedAmslM: pos.altitude.meters,
    relativeAltM: pos.relative_alt_m,
  })
}

/** DISTANCE_SENSOR current, cm→m, `null` when absent/stale. */
function distanceSensorAgl(
  snap: import('../generated-types/TelemetrySnapshot').TelemetrySnapshot,
): number | null {
  const ds = snap.distance_sensor
  if (!ds) return null
  const ageMs = snap.field_ages?.distance_sensor_at_ms
  if (ageMs != null && Date.now() - ageMs > 3000) return null
  return ds.current_m
}
