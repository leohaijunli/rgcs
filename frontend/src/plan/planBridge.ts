// Flight-plan window bridge (ADR-016 pattern, bidirectional): the mission
// store lives in the MAIN window; this window renders a read-only mirror of
// it and sends edit intents back. The main window applies the intents to its
// store and broadcasts fresh snapshots — the mirror is always a projection,
// never a second source of truth.
//
// Snapshot:  main --plan_state {rev, state}--> plan
// Edit:      plan --plan_edit {action, args}--> main

import { listen } from '@tauri-apps/api/event'

/** The mission-state slice the plan window mirrors (JSON-safe subset). */
export interface PlanSnapshot {
  rev: number
  waypoints: unknown[]
  blocks: unknown[]
  home: [number, number, number] | null
  patternCenter: { latitude_deg: number; longitude_deg: number } | null
  lastPattern: unknown
  drape: unknown
  altitudeMode: string
  surveyAglM: number
  selectedSeq: number | null
  busy: boolean
  dirty: boolean
  syncState: string
  fcMatches: boolean | null
  /** Survey polygon (drawn on the main window's map) for the Area step. */
  polygon: { vertices: unknown[]; closed: boolean }
  /** Main window's map centre — the default position for new waypoints. */
  mapCenter: { lat: number; lon: number } | null
}

let started = false
let lastRev = -1
type SnapshotListener = (s: PlanSnapshot) => void
const listeners = new Set<SnapshotListener>()

export function isPlanWindow(): boolean {
  return new URLSearchParams(window.location.search).has('plan') ||
    window.location.pathname.endsWith('plan.html')
}

/** Subscribe to mission snapshots from the main window (idempotent). */
export function startPlanBridge(): void {
  if (started) return
  started = true
  void (async () => {
    try {
      await listen<PlanSnapshot>('plan_state', (e) => {
        if (e.payload.rev <= lastRev) return // stale replay
        lastRev = e.payload.rev
        for (const fn of listeners) fn(e.payload)
      })
      // Ask the main window for a fresh snapshot (boot / reload).
      const { emit } = await import('@tauri-apps/api/event')
      await emit('plan_ready', {})
    } catch {
      /* events unavailable (browser dev) */
    }
  })()
}

export function onPlanSnapshot(fn: SnapshotListener): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

/** Send an edit intent to the main window. */
export async function sendPlanEdit(action: string, args: Record<string, unknown> = {}): Promise<void> {
  const { emit } = await import('@tauri-apps/api/event')
  await emit('plan_edit', { action, args })
}
