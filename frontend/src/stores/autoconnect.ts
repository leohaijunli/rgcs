// The auto-connect confirmation (L8): autoConnect used to fire the `connect`
// command silently at startup, so a changed endpoint (or a stale config from
// another vehicle) connected before the operator saw it. The bridge now
// *requests* a connect; this store holds the pending request and starts the
// 3 s window itself — expiry fires the connect even if the dialog is not on
// screen (a start page or another stage must not stall the connect forever).
// The dialog is pure display over this state: "connect to X?" with a Cancel.

import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { reportConnectError } from '../desktop/connect'

/** How long the operator has to cancel before the connect fires (plan L8). */
export const AUTO_CONNECT_WINDOW_MS = 3000

interface AutoConnectState {
  /** The endpoint waiting to be connected; `null` = nothing pending. */
  pendingEndpoint: string | null
  /** Wall-clock deadline the countdown renders against. */
  deadlineMs: number
  request: (endpoint: string) => void
  cancel: () => void
}

let timer: ReturnType<typeof setTimeout> | undefined

function fire(endpoint: string): void {
  void invoke('connect', { endpoint }).catch(reportConnectError)
}

export const useAutoConnectStore = create<AutoConnectState>((set) => ({
  pendingEndpoint: null,
  deadlineMs: 0,
  request: (endpoint) => {
    if (timer !== undefined) clearTimeout(timer)
    set({ pendingEndpoint: endpoint, deadlineMs: Date.now() + AUTO_CONNECT_WINDOW_MS })
    timer = setTimeout(() => {
      timer = undefined
      set({ pendingEndpoint: null, deadlineMs: 0 })
      fire(endpoint)
    }, AUTO_CONNECT_WINDOW_MS)
  },
  cancel: () => {
    if (timer !== undefined) clearTimeout(timer)
    timer = undefined
    set({ pendingEndpoint: null, deadlineMs: 0 })
  },
}))

// Dev-only handle for the Playwright checks (same pattern as `__mgMission`):
// the 3 s window starts at boot, so the checks drive the store directly
// instead of racing the initial page load.
if (import.meta.env?.DEV && typeof window !== 'undefined') {
  ;(window as unknown as { __mgAutoConnect?: unknown }).__mgAutoConnect =
    useAutoConnectStore
}
