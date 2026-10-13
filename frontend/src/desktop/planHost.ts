// Main-window side of the flight-plan window bridge (see
// `src/plan/planBridge.ts`): broadcast mission snapshots to the `plan`
// window and apply the edit intents it sends back. The mission store stays
// resident HERE — the plan window is a mirror plus intent sender.

import { listen, emit } from '@tauri-apps/api/event'
import { useMissionStore } from '../stores/mission'
import type { PlanSnapshot } from '../plan/planBridge'

/** Milliseconds to settle store churn before broadcasting a snapshot. */
const BROADCAST_DEBOUNCE_MS = 120

let started = false
let rev = 0
let timer: ReturnType<typeof setTimeout> | undefined

function snapshot(): PlanSnapshot {
  const s = useMissionStore.getState()
  return {
    rev: ++rev,
    waypoints: s.waypoints,
    blocks: s.blocks,
    home: s.home,
    patternCenter: s.patternCenter,
    lastPattern: s.lastPattern,
    drape: s.drape,
    altitudeMode: s.altitudeMode,
    surveyAglM: s.surveyAglM,
    selectedSeq: s.selectedSeq,
    busy: s.busy,
    dirty: s.dirty,
    syncState: s.syncState,
    fcMatches: s.fcMatches,
  }
}

function broadcast(): void {
  if (timer !== undefined) clearTimeout(timer)
  timer = setTimeout(() => {
    void emit('plan_state', snapshot()).catch(() => {})
  }, BROADCAST_DEBOUNCE_MS)
}

/** Apply one edit intent from the plan window to the resident store. */
function applyEdit(action: string, args: Record<string, unknown>): void {
  const s = useMissionStore.getState()
  switch (action) {
    case 'insertPattern':
      s.insertPattern(
        args.plan as never,
        args.label as string,
        (args.center as { latitude_deg: number; longitude_deg: number } | null) ?? null,
        (args.stats as never) ?? null,
      )
      break
    case 'clearPlan':
      s.clearPlan()
      break
    case 'setAltitudeMode':
      s.setAltitudeMode(args.mode as never)
      break
    case 'setSurveyAglM':
      s.setSurveyAglM(args.aglM as number)
      break
    case 'setPatternCenter':
      s.setPatternCenter((args.center as never) ?? null)
      break
    case 'moveWaypoint':
      s.moveWaypoint(args.from as number, args.to as number)
      break
    case 'removeWaypoint':
      s.removeWaypoint(args.seq as number)
      break
    case 'upload':
      void s.upload()
      break
    default:
      break // unknown intent from a newer plan window: ignore
  }
}

/** Idempotently start the main-window side of the bridge. */
export function startPlanHost(): void {
  if (started) return
  started = true
  // Any mission-store change re-broadcasts (debounced); the plan window
  // applies snapshots by rev, so this is lossless and loop-free.
  useMissionStore.subscribe(broadcast)
  void (async () => {
    try {
      // The plan window asks for a fresh snapshot on boot/reload.
      await listen('plan_ready', () => broadcast())
      await listen<{ action: string; args: Record<string, unknown> }>('plan_edit', (e) => {
        applyEdit(e.payload.action, e.payload.args ?? {})
      })
    } catch {
      /* events unavailable (browser dev) */
    }
  })()
}
