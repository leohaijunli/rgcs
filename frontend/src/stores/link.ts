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
  setLink: (link) => set({ link }),
  setError: (lastError) => set({ lastError }),
  pushError: (e) =>
    set((state) => ({
      lastError: e.message,
      errorHistory: [e, ...state.errorHistory].slice(0, MAX_ERROR_HISTORY),
    })),
  setDroppedFrames: (droppedFrames) => set({ droppedFrames }),
  clearErrors: () => set({ lastError: null, errorHistory: [] }),
}))
