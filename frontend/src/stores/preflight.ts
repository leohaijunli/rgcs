// Preflight checklist (L5): maps the frontend mission-sync state onto the
// backend's `PlanState` and fetches the evaluated checklist. The arm gate is
// authoritative in core (`send_arm_disarm`); this module only renders it and
// reproduces the gate shape for the one-click Takeoff button (a pure display
// rule — the backend still enforces the arm gate itself).

import { invoke } from '@tauri-apps/api/core'
import { create } from 'zustand'
import type { Checklist } from '../generated-types/Checklist'
import type { ChecklistStatus } from '../generated-types/ChecklistStatus'
import type { DrapeCheckFacts } from '../generated-types/DrapeCheckFacts'
import type { PlanState } from '../generated-types/PlanState'
import { getSafetyConfig } from '../desktop/appConfig'
import type { DrapePreview } from '../mission/drapePreview'
import { useMissionStore } from './mission'

/** The client-side view of the gate (mirrors `core::safety::arm_gate`). */
export type GateStatus = 'allowed' | 'requires_ack' | 'blocked' | 'no_link'

/** Frontend mission-sync facts for the backend gate (L5). The motor-test bit
 * is overridden by the backend, which owns that state. */
export function buildPlanState(): PlanState {
  const m = useMissionStore.getState()
  const hasPlan = m.waypoints.length > 0 || m.blocks.length > 0
  // The plan matches the FC when it was not edited since the last sync and no
  // intentional clear is waiting to be re-downloaded.
  const inSync = !m.dirty && !m.planCleared && m.syncState === 'idle'
  const mission_uploaded = inSync && hasPlan
  const mission_verified = m.fcMatches === true && inSync && hasPlan
  // The Drape checklist facts apply only when the planned route is actually
  // ON the FC (operator decision 2026-10-18: not uploading a mission must
  // not prohibit arming — the Drape group would otherwise block a manual
  // flight on facts about a route the FC will not fly).
  const drape: DrapeCheckFacts | null =
    mission_verified && m.drapePreview != null ? drapeFacts(m.drapePreview) : null
  return {
    has_plan: hasPlan,
    mission_uploaded,
    mission_verified,
    motor_test_active: false,
    drape,
  }
}

/** Map the solver preview onto the checklist facts (DR4). The out-of-band
 * spans pass only when the operator accepted *this* solve (U5): a new solve
 * (different `spec_hash`) invalidates the acceptance by comparison. */
function drapeFacts(preview: DrapePreview): DrapeCheckFacts {
  const m = useMissionStore.getState()
  const ageYears = preview.acquisition
    ? (Date.now() - Date.parse(preview.acquisition)) / (365.25 * 24 * 3600 * 1000)
    : null
  const limit = getSafetyConfig().dem_max_age_years
  return {
    dem_covers_route: preview.covers_route,
    has_dtm: preview.has_dtm,
    dem_age_ok: ageYears != null && limit != null ? ageYears <= limit : null,
    infeasible_count: preview.result.infeasible.length,
    infeasible_confirmed: m.infeasibleAcceptedFor === preview.result.spec_hash,
    waypoint_count: preview.result.compiled.length,
    waypoint_limit: 65_535,
    max_climb_needed_mps: preview.result.stats.max_climb_needed_mps,
    fc_max_climb_mps: null, // the FC climb parameter (*verify*, issue 13)
    battery_minutes: null,
    eta_minutes: null,
  }
}

/** Evaluate the checklist in the backend and return it. */
export async function fetchChecklist(): Promise<Checklist> {
  return invoke<Checklist>('preflight_checklist', { planState: buildPlanState() })
}

/** The gate shape for the Takeoff button (a display rule mirroring
 * `core::safety::arm_gate`): a stale link → `no_link` (nothing can be sent);
 * any `Fail`/`Unknown` item → `blocked`; else `Warn` items → `requires_ack`;
 * otherwise `allowed`. */
export function gateStatus(checklist: Checklist): GateStatus {
  const link = checklist.items.find((i) => i.id === 'link_fresh')
  if (!link || link.status !== 'pass') return 'no_link'
  if (checklist.items.some((i) => i.status === 'fail' || i.status === 'unknown')) return 'blocked'
  if (checklist.items.some((i) => i.status === 'warn')) return 'requires_ack'
  return 'allowed'
}

/** Overall verdict derived from the checklist (U5's flyable badge). */
export function overallOf(checklist: Checklist): ChecklistStatus {
  if (checklist.items.some((i) => i.status === 'fail')) return 'fail'
  if (checklist.items.some((i) => i.status === 'unknown')) return 'unknown'
  if (checklist.items.some((i) => i.status === 'warn')) return 'warn'
  return 'pass'
}

/** Panel-open state. The panel is also auto-opened by a gate refusal (L5). */
interface PreflightUiState {
  open: boolean
  openPanel: () => void
  closePanel: () => void
}

export const usePreflightUi = create<PreflightUiState>((set) => ({
  open: false,
  openPanel: () => set({ open: true }),
  closePanel: () => set({ open: false }),
}))