import { create } from 'zustand'
import type { LinkStatus } from '../generated-types/LinkStatus'
import type { TelemetryError } from '../generated-types/TelemetryError'

/** Cap the in-memory link error history (issue #17). */
const MAX_ERROR_HISTORY = 50

interface LinkState {
  link: LinkStatus | null
  lastError: string | null
  errorHistory: TelemetryError[]
  droppedFrames: number
  /**
   * Epoch ms of the last lost->alive transition (L3), `null` before the first
   * one. The UI shows a "link restored" notice for a few seconds after it.
   */
  restoredAtMs: number | null
  setLink: (l: LinkStatus) => void
  setError: (e: string | null) => void
  pushError: (e: TelemetryError) => void
  setDroppedFrames: (n: number) => void
  clearErrors: () => void
}

export const useLinkStore = create<LinkState>((set) => ({
  link: null,
  lastError: null,
  errorHistory: [],
  droppedFrames: 0,
  restoredAtMs: null,
  // The lost->alive edge is the only place that knows a *reconnect* happened
  // (a plain `link` event carries no history), so the timestamp is stamped
  // here and every view — banner, mission re-read, event log — reads it.
  setLink: (link) =>
    set((state) => ({
      link,
      restoredAtMs:
        link.fc_alive && state.link?.fc_alive === false ? Date.now() : state.restoredAtMs,
    })),
  setError: (lastError) => set({ lastError }),
  pushError: (e) =>
    set((state) => ({
      lastError: e.message,
      errorHistory: [e, ...state.errorHistory].slice(0, MAX_ERROR_HISTORY),
    })),
  setDroppedFrames: (droppedFrames) => set({ droppedFrames }),
  clearErrors: () => set({ lastError: null, errorHistory: [] }),
}))
