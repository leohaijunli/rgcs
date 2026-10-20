import { create } from 'zustand'
import type { TelemetrySnapshot } from '../generated-types/TelemetrySnapshot'

interface TelemetryState {
  snapshot: TelemetrySnapshot | null
  /** Monotonic ms of the last applied snapshot (for staleness). */
  lastUpdateAtMs: number
  /** True when the mock feed is the data source (browser dev). */
  isMock: boolean
  applySnapshot: (s: TelemetrySnapshot) => void
  setMock: (m: boolean) => void
}

export const useTelemetryStore = create<TelemetryState>((set) => ({
  snapshot: null,
  lastUpdateAtMs: 0,
  isMock: false,
  applySnapshot: (snapshot) =>
    set({ snapshot, lastUpdateAtMs: typeof performance !== 'undefined' ? performance.now() : 0 }),
  setMock: (isMock) => set({ isMock }),
}))