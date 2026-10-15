// Preflight checklist (L5): maps the frontend mission-sync state onto the
// backend's `PlanState` and fetches the evaluated checklist. The arm gate is
// authoritative in core (`send_arm_disarm`); this module only renders it and
// reproduces the gate shape for the one-click Takeoff button (a pure display
// rule — the backend still enforces the arm gate itself).

import { invoke } from '@tauri-apps/api/core'
import { create } from 'zustand'
import type { Checklist } from '../generated-types/Checklist'
import type { ChecklistStatus } from '../generated-types/ChecklistStatus'
import type { PlanState } from '../generated-types/PlanState'
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
  return {
    has_plan: hasPlan,
    mission_uploaded: inSync && hasPlan,
    mission_verified: m.fcMatches === true && inSync && hasPlan,
    motor_test_active: false,
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