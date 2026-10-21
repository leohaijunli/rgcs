// The link-target comparison state (L8): holds the pending sysid/vehicle
// change for the confirmation dialog and remembers which link session was
// already checked (a lost→alive edge re-runs the check via `restoredAtMs`).

import { create } from 'zustand'
import type { LastTarget } from '../generated-types/LastTarget'

export interface TargetChange {
  previous: LastTarget | null
  current: LastTarget
}

interface TargetCheckState {
  /** The unconfirmed change, `null` once acknowledged or disconnected. */
  change: TargetChange | null
  /** The link session (`restoredAtMs`) already evaluated. */
  checkedSession: number | null
  dismiss: () => void
}

export const useTargetCheckStore = create<TargetCheckState>((set) => ({
  change: null,
  checkedSession: null,
  // Reset the session sentinel too: the next heartbeat re-runs the
  // comparison (confirm makes it a silent same-target match; a refused
  // change simply stops — the link is gone).
  dismiss: () => set({ change: null, checkedSession: null }),
}))
