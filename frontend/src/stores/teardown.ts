// One confirm dialog for every path that stops the link or the app (L4).
//
// The three in-app exit points (link popover, Settings ▸ Disconnect, Settings ▸
// Shut down) open *this* request instead of calling the shell directly, so the
// refusal, the typed keyword and the "what would be lost" line are written
// once. The window close button is not one of them: the shell forces that one
// (plan §4 L4) and records it as an alert.

import { create } from 'zustand'
import type { StopReason } from '../generated-types/StopReason'

interface TeardownState {
  /** The teardown waiting for confirmation; `null` when the dialog is closed. */
  pending: StopReason | null
  request: (reason: StopReason) => void
  close: () => void
}

export const useTeardownStore = create<TeardownState>((set) => ({
  pending: null,
  request: (pending) => set({ pending }),
  close: () => set({ pending: null }),
}))
