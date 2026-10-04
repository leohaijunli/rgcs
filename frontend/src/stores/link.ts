import { create } from 'zustand'
import type { LinkStatus } from '../generated-types/LinkStatus'

interface LinkState {
  link: LinkStatus | null
  lastError: string | null
  setLink: (l: LinkStatus) => void
  setError: (e: string | null) => void
}

export const useLinkStore = create<LinkState>((set) => ({
  link: null,
  lastError: null,
  setLink: (link) => set({ link }),
  setError: (lastError) => set({ lastError }),
}))