// Bridges the desktop shell and the UI.
//
// Inside Tauri: subscribes to Rust-pushed events (`telemetry`, `link`,
// `link_error`, `alert`) and calls commands (`connect`, `disconnect`,
// `link_status`).
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
import type { TelemetryError } from '../generated-types/TelemetryError'
import type { MissionEventPayload } from '../stores/mission'
import type { AlertEvent } from '../generated-types/AlertEvent'
import { loadAlerts, useAlerts } from '../stores/alerts'
import { reportConnectError } from './connect'
import { syncSitlHome, usePrefsStore } from './prefs'
import type { CommandEventPayload } from '../stores/command'
import { useCommandStore } from '../stores/command'
import { useDevicesStore } from '../stores/devices'
import { useLinkStore } from '../stores/link'
import { useMissionStore } from '../stores/mission'
import { useTelemetryStore } from '../stores/telemetry'
import { startMockFeed } from '../telemetry/mock'
import { startDeviceWatcher } from '../stores/devices'

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
      // Serial hotplug subscription (issues.md #40): keeps the connection
      // dropdown live when ports appear or disappear.
      startDeviceWatcher()
      const un1 = await listen<TelemetrySnapshot>('telemetry', (e) => {
        useTelemetryStore.getState().applySnapshot(e.payload)
      })
      if (disposed) return un1()
      unlisteners.push(un1)

      const un2 = await listen<LinkStatus>('link', (e) => {
        useLinkStore.getState().setLink(e.payload)
        // A dead link ends any in-flight command or mission operation; their
        // acks/events will never arrive (the mission service exits with the
        // connection), so clear the pending state instead of hanging.
        if (!e.payload.fc_alive) {
          useCommandStore.getState().reset()
          useMissionStore.getState().linkLost()
        }
      })
      if (disposed) return un2()
      unlisteners.push(un2)

      const un3 = await listen<TelemetryError>('link_error', (e) => {
        useLinkStore.getState().pushError(e.payload)
      })
      if (disposed) return un3()
      unlisteners.push(un3)

      const un3b = await listen<number>('telemetry_dropped', (e) => {
        useLinkStore.getState().setDroppedFrames(e.payload)
      })
      if (disposed) return un3b()
      unlisteners.push(un3b)

      // Alert center (A3): one `alert` stream for every window; the badge,
      // banner and drawer all read the store it feeds.
      const un3c = await listen<AlertEvent>('alert', (e) => {
        useAlerts.getState().apply(e.payload)
      })
      if (disposed) return un3c()
      unlisteners.push(un3c)

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

      // Alerts raised before this webview booted (the badge must survive a
      // window reload).
      await loadAlerts().catch(() => {})

      if (!connectStarted) {
        connectStarted = true
        // Report auto-connect failures in the status bar instead of hiding
        // them (issue #4). The endpoint and the toggle are remembered across
        // restarts (Phase 0 addendum, task 0.8).
        const { endpoint, autoConnect } = usePrefsStore.getState()
        // Keep the SITL helper file in step with a setting saved by an older
        // build or on a fresh profile (Settings -> Vehicle).
        syncSitlHome(usePrefsStore.getState().initialPosition)
        if (autoConnect) {
          await invoke('connect', { endpoint }).catch(reportConnectError)
        }
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
