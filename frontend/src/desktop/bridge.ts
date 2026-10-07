// Bridges the desktop shell and the UI.
//
// Inside Tauri: subscribes to Rust-pushed events (`telemetry`, `link`,
// `link_error`) and calls commands (`connect`, `disconnect`, `link_status`).
// In a plain browser (Vite dev): starts the mock feed so the layout stays
// alive until the desktop shell is built.
//
// The async `setup()` guards against React StrictMode double-mounts: every
// await checks a `disposed` flag so listeners registered after an early
// cleanup are unregistered immediately, and auto-connect runs once per load.

import { useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { LinkStatus } from '../generated-types/LinkStatus'
import type { TelemetrySnapshot } from '../generated-types/TelemetrySnapshot'
import type { MissionItem } from '../generated-types/MissionItem'
import type { MissionEventPayload } from '../stores/mission'
import { reportConnectError } from './connect'
import type { CommandEventPayload } from '../stores/command'
import { useCommandStore } from '../stores/command'
import { useDevicesStore } from '../stores/devices'
import { useLinkStore } from '../stores/link'
import { useMissionStore } from '../stores/mission'
import { useTelemetryStore } from '../stores/telemetry'
import { startMockFeed } from '../telemetry/mock'

const DEFAULT_ENDPOINT = 'udpin:0.0.0.0:14550'

function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

let connectStarted = false

export function useTelemetryBridge() {
  useEffect(() => {
    let disposed = false
    const unlisteners: Array<() => void> = []
    let mockTimer: number | undefined

    async function setup() {
      if (!inTauri()) {
        mockTimer = startMockFeed()
        return
      }
      const un1 = await listen<TelemetrySnapshot>('telemetry', (e) => {
        useTelemetryStore.getState().applySnapshot(e.payload)
      })
      if (disposed) return un1()
      unlisteners.push(un1)

      const un2 = await listen<LinkStatus>('link', (e) => {
        useLinkStore.getState().setLink(e.payload)
        // A dead link ends any in-flight command; its ack will never arrive.
        if (!e.payload.fc_alive) useCommandStore.getState().reset()
      })
      if (disposed) return un2()
      unlisteners.push(un2)

      const un3 = await listen<string>('link_error', (e) => {
        useLinkStore.getState().setError(e.payload)
      })
      if (disposed) return un3()
      unlisteners.push(un3)

      const un4 = await listen<MissionEventPayload>('mission', (e) => {
        useMissionStore.getState().handleEvent(e.payload)
      })
      if (disposed) return un4()
      unlisteners.push(un4)

      const un5 = await listen<MissionItem[]>('mission_plan', (e) => {
        useMissionStore.getState().handlePlan(e.payload)
      })
      if (disposed) return un5()
      unlisteners.push(un5)

      const un6 = await listen<CommandEventPayload>('command', (e) => {
        useCommandStore.getState().handleEvent(e.payload)
      })
      if (disposed) return un6()
      unlisteners.push(un6)

      const snap = await invoke<LinkStatus | null>('link_status')
      if (disposed) return
      if (snap) useLinkStore.getState().setLink(snap)

      if (!connectStarted) {
        connectStarted = true
        // Report auto-connect failures in the status bar instead of hiding
        // them (issue #4).
        await invoke('connect', { endpoint: DEFAULT_ENDPOINT }).catch(reportConnectError)
      }
      void useDevicesStore.getState().load()
    }

    void setup()
    return () => {
      disposed = true
      unlisteners.forEach((u) => u())
      if (mockTimer !== undefined) window.clearInterval(mockTimer)
    }
  }, [])
}
