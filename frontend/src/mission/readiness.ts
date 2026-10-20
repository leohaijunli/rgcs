// Plan readiness (U17): the single source of the step-by-step panel's
// navigation state — the status dot on each step card and, later, the blocker
// list that jumps back to the field that has to change. Derived from the same
// facts the stores already expose, so the dots, the blockers and the summary
// strip cannot drift apart.
//
// Pure and dependency-free (only the sync classification's *type*), so
// `check:plan-steps` can bundle it without the Tauri runtime.

import type { PlanSyncStatus } from './sync'

/** The four planning steps, in workflow order. */
export type PlanStepId = 'area' | 'pattern' | 'waypoints' | 'sync'

/**
 * Per-step navigation state. `blocked` is the one that must not be missed:
 * the workflow cannot be trusted until the operator fixes it (a failed terrain
 * drape, an FC mission that differs from what we uploaded).
 */
export type StepStatus = 'todo' | 'done' | 'blocked'

export interface PlanReadinessFacts {
  /** A usable boundary exists: a closed, non-self-intersecting polygon, or a
   *  rectangle centre (the map view's centre counts as a fallback). */
  hasArea: boolean
  /** A trajectory has been generated (`lastPattern`). */
  hasPattern: boolean
  /** Flyable item count (editable waypoints + complex-item children). */
  itemCount: number
  /** Sync classification (`plan.sync`). */
  sync: PlanSyncStatus
  /** Non-null when the terrain drape failed — heights are not
   *  terrain-following and the plan must not be flown as-is. */
  drapeError: string | null
}

export interface PlanStepState {
  id: PlanStepId
  status: StepStatus
}

/** A condition that must be fixed before the plan can be trusted. */
export type BlockerCode = 'drape_failed' | 'fc_mismatch'

export interface PlanBlocker {
  /** The step card whose fix clears the blocker (it shows the blocked dot). */
  step: PlanStepId
  /** Field to reveal after navigating there (`data-plan-field`). */
  field: string
  code: BlockerCode
  /** Optional raw detail (e.g. the drape error string) shown next to the text. */
  detail?: string
}

/**
 * The blocking conditions, in the order the operator should clear them. Text
 * lives in `plan.blocker.*` (this module stays i18n-free); the UI turns each
 * into a "go fix it" chip that opens `step` and highlights `field`.
 */
export function planBlockers(f: PlanReadinessFacts): PlanBlocker[] {
  const out: PlanBlocker[] = []
  // A failed drape is fixed where the drape is reported/run — the Pattern step.
  if (f.drapeError) {
    out.push({ step: 'pattern', field: 'drape', code: 'drape_failed', detail: f.drapeError })
  }
  // An FC mission that differs from the uploaded plan is fixed in Sync.
  if (f.itemCount > 0 && f.sync === 'mismatch') {
    out.push({ step: 'sync', field: 'sync', code: 'fc_mismatch' })
  }
  return out
}

/**
 * Derive each step's status, in workflow order. The list always covers all
 * four steps; the caller drops `area` for a cloverleaf (no boundary) and
 * renumbers the rest. Blocked comes straight from `planBlockers`, so the dot
 * and the chip can never point at different steps.
 */
export function planSteps(f: PlanReadinessFacts): PlanStepState[] {
  const blocked = new Set(planBlockers(f).map((b) => b.step))
  const status = (id: PlanStepId, done: boolean): StepStatus =>
    blocked.has(id) ? 'blocked' : done ? 'done' : 'todo'
  return [
    { id: 'area', status: status('area', f.hasArea) },
    { id: 'pattern', status: status('pattern', f.hasPattern) },
    { id: 'waypoints', status: status('waypoints', f.itemCount > 0) },
    { id: 'sync', status: status('sync', f.itemCount > 0 && f.sync === 'synced') },
  ]
}
