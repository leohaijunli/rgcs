// Persisted connection preferences (DEVELOPMENT_PLAN Phase 0 addendum, task 0.8):
// the last endpoint and the auto-connect toggle survive a restart.

import { create } from 'zustand'
import { DEFAULT_ENDPOINT } from './endpoint'

const ENDPOINT_KEY = 'maggcs.endpoint'
const AUTOCONNECT_KEY = 'maggcs.autoconnect'

function read(key: string, fallback: string): string {
  try {
    return localStorage.getItem(key) ?? fallback
  } catch {
    return fallback
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value)
  } catch {
    /* private mode */
  }
}

interface PrefsState {
  endpoint: string
  autoConnect: boolean
  setEndpoint: (endpoint: string) => void
  setAutoConnect: (autoConnect: boolean) => void
}

export const usePrefsStore = create<PrefsState>((set) => ({
  endpoint: read(ENDPOINT_KEY, DEFAULT_ENDPOINT),
  autoConnect: read(AUTOCONNECT_KEY, 'true') !== 'false',
  setEndpoint: (endpoint) => {
    write(ENDPOINT_KEY, endpoint)
    set({ endpoint })
  },
  setAutoConnect: (autoConnect) => {
    write(AUTOCONNECT_KEY, String(autoConnect))
    set({ autoConnect })
  },
}))
