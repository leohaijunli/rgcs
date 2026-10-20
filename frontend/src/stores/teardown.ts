// One confirm dialog for every path that stops the link or the app (L4).
//
// In-app exit points open *this* request instead of calling the shell
// directly. A window-close refusal is a notification in the same store, since
// the close originates in native chrome rather than a frontend button.

import { create } from 'zustand'
import type { StopReason } from '../generated-types/StopReason'

interface TeardownState {
  /** The teardown waiting for confirmation; `null` when the dialog is closed. */
  pending: StopReason | null
  request: (reason: StopReason) => void
  close: () => void
  closeRefusal: string | null
  reportCloseRefusal: (reason: string) => void
  clearCloseRefusal: () => void
}

export const useTeardownStore = create<TeardownState>((set) => ({
  pending: null,
  request: (pending) => set({ pending }),
  close: () => set({ pending: null }),
  closeRefusal: null,
  reportCloseRefusal: (closeRefusal) => set({ closeRefusal }),
  clearCloseRefusal: () => set({ closeRefusal: null }),
}))
