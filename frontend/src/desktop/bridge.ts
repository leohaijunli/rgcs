// Bridges the desktop shell and the UI.
//
// Inside Tauri: subscribes to Rust-pushed events (`telemetry`, `link`,
// `link_error`, `alert`) and calls commands (`connect`, `disconnect`,
// `link_status`).
// In a plain browser (Vite dev): starts the mock feed so the layout stays
// alive until the desktop shell is built.
//
// `statustext` (L1) is the FC's own log line: it lands in the event log with
// the FC's severity, while the alarming ones additionally become alerts in
// Rust (badge, banner, drawer) — the same words in both surfaces.
//
// The async `setup()` guards against React StrictMode double-mounts: every
// await checks a `disposed` flag so listeners registered after an early
// cleanup are unregistered immediately, and auto-connect runs once per load.

import { useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useTeardownStore } from '../stores/teardown'
import type { LinkStatus } from '../generated-types/LinkStatus'
import type { TelemetrySnapshot } from '../generated-types/TelemetrySnapshot'
import type { MissionProgressEnvelope } from '../generated-types/MissionProgressEnvelope'
import type { MissionCurrentSnapshot } from '../generated-types/MissionCurrentSnapshot'
import type { OperationStatus } from '../generated-types/OperationStatus'
import type { TelemetryError } from '../generated-types/TelemetryError'
import i18n from '../i18n'
import { logEvent } from '../stores/eventlog'
import { syncSitlHome, usePrefsStore } from './prefs'
import { useCommandStore } from '../stores/command'
import { useConsoleStore } from '../stores/console'
import { useLogsStore } from '../stores/logs'
import { useDevicesStore } from '../stores/devices'
import { useLinkStore } from '../stores/link'
import { useMissionStore } from '../stores/mission'
import { useRtkStore } from '../stores/rtk'
import { useTelemetryStore } from '../stores/telemetry'
import { startMockFeed } from '../telemetry/mock'
import { startDeviceWatcher } from '../stores/devices'
import { startSerialAutoWatcher, useSerialAutoStore } from '../stores/serialAuto'
import type { SerialAutoConnectSnapshot } from '../generated-types/SerialAutoConnectSnapshot'
import { legacyRtkSettings } from '../util/rtkSettings'

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
      const closeRefusalUnlisten = await listen<string>('teardown_refused', (event) => {
        useTeardownStore.getState().reportCloseRefusal(event.payload)
      })
      if (disposed) return closeRefusalUnlisten()
      unlisteners.push(closeRefusalUnlisten)
      const pendingCloseRefusal = await invoke<string | null>('take_teardown_refusal')
      if (disposed) return
      if (pendingCloseRefusal !== null) {
        useTeardownStore.getState().reportCloseRefusal(pendingCloseRefusal)
      }

      // Serial hotplug subscription (issues.md #40): keeps the connection
      // dropdown live when ports appear or disappear.
      startDeviceWatcher()
      startSerialAutoWatcher()
      const un1 = await listen<TelemetrySnapshot>('telemetry', (e) => {
        const previous = useTelemetryStore.getState().snapshot?.heartbeat
        const next = e.payload.heartbeat
        if (
          previous
          && next
          && (previous.system_id !== next.system_id || previous.component_id !== next.component_id)
        ) {
          useCommandStore.getState().reset()
          useMissionStore.getState().linkLost()
          useConsoleStore.getState().reset()
          useLogsStore.getState().reset()
        }
        useTelemetryStore.getState().applySnapshot(e.payload)
      })
      if (disposed) return un1()
      unlisteners.push(un1)

      const un2 = await listen<LinkStatus>('link', (e) => {
        // The lost->alive edge is the reconnect (L3): read it *before* the
        // store is updated, because `setLink` is what consumes it.
        const previous = useLinkStore.getState().link
        const wasAlive = previous?.fc_alive
        if (
          previous
          && (previous.endpoint !== e.payload.endpoint
            || previous.link_state !== e.payload.link_state
            || previous.fc_alive !== e.payload.fc_alive)
        ) {
          useCommandStore.getState().reset()
          useMissionStore.getState().linkLost()
          useConsoleStore.getState().reset()
          useLogsStore.getState().reset()
        }
        useLinkStore.getState().setLink(e.payload)
        // A dead link ends any in-flight command or mission operation; their
        // acks/events will never arrive (the mission service exits with the
        // connection), so clear the pending state instead of hanging.
        if (e.payload.link_state !== 'connected') {
          // A vehicle disconnect retires forwarding, while the independent
          // base receiver can continue its survey and RTCM output.
          useRtkStore.getState().resetLink()
        }
        if (!e.payload.fc_alive) {
          useCommandStore.getState().reset()
          useMissionStore.getState().linkLost()
          useConsoleStore.getState().reset()
          useLogsStore.getState().reset()
        } else if (wasAlive === false) {
          // Reconnect: mode and armed state resume with the heartbeat; the
          // mission state is only known from an exchange, so re-read it, and
          // say so — a banner that vanishes silently leaves the operator
          // wondering whether the panel is live again.
          useMissionStore.getState().linkRestored()
          logEvent('link', 'ok', i18n.t('link.restored'))
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

      // A3/U3: the alert center and the FC status console are wired by
      // `useAlertStream()` in the shared alert layer, so the main window and
      // the tool windows all get the same `alert` + `statustext` stream.

      const un4 = await listen<OperationStatus>('mission_operation', (e) => {
        useMissionStore.getState().handleOperation(e.payload)
      })
      if (disposed) return un4()
      unlisteners.push(un4)

      const un5 = await listen<MissionProgressEnvelope>('mission_progress', (e) => {
        useMissionStore.getState().handleProgress(e.payload)
      })
      if (disposed) return un5()
      unlisteners.push(un5)
      const un6 = await listen<MissionCurrentSnapshot>('mission_current', (e) => {
        useMissionStore.getState().handleCurrent(e.payload)
      })
      if (disposed) return un6()
      unlisteners.push(un6)

      const snap = await invoke<LinkStatus | null>('link_status')
      if (disposed) return
      if (snap) useLinkStore.getState().setLink(snap)

      if (!connectStarted) {
        connectStarted = true
        // Report auto-connect failures in the status bar instead of hiding
        // them (issue #4). The endpoint and the toggle are remembered across
        // restarts (Phase 0 addendum, task 0.8).
        const { endpoint, autoConnect } = usePrefsStore.getState()
        // Keep the SITL helper file in step with a setting saved by an older
        // build or on a fresh profile (Settings -> Vehicle).
        syncSitlHome(usePrefsStore.getState().initialPosition)
        const serial = await invoke<SerialAutoConnectSnapshot | null>('serial_auto_connect_status', { legacyRtk: legacyRtkSettings() }).catch(() => null)
        if (disposed) return
        useSerialAutoStore.getState().handle(serial)
        if (autoConnect && (serial?.primary_startup_owner === 'legacy_endpoint' || (!serial && !endpoint.startsWith('serial:')))) {
          // L8: the operator gets a 3 s cancel window instead of a silent
          // connect — the dialog fires `connect` when the countdown expires.
          const { useAutoConnectStore } = await import('../stores/autoconnect')
          useAutoConnectStore.getState().request(endpoint)
        }
      }
      void useDevicesStore.getState().load()
    }

    void setup()
    return () => {
      disposed = true
      unlisteners.forEach((u) => u())
      useCommandStore.getState().reset()
      useMissionStore.getState().linkLost()
      useConsoleStore.getState().reset()
      if (mockTimer !== undefined) window.clearInterval(mockTimer)
    }
  }, [])
}
