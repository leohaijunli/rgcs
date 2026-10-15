// Alert center store (plan §3 A3): the single frontend view of
// `core::alerts::AlertCenter`. The TopBar badge, the banner stack and the
// event-log drawer all read this store, so "how many alerts need attention"
// and "how severe is the worst one" have exactly one answer everywhere.
//
// Rust owns the model — dedup, latching, ordering and capacity all live in
// core, and every transition arrives as an `alert` event carrying the alert's
// state *after* the change. This store only mirrors it.

import { create } from 'zustand'
import { useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { Alert } from '../generated-types/Alert'
import type { AlertEvent } from '../generated-types/AlertEvent'
import type { Severity } from '../generated-types/Severity'
import type { AlertSource } from '../generated-types/AlertSource'
import type { StatusText } from '../generated-types/StatusText'
import { logEvent, type EventLevel } from './eventlog'

/** Severity order for the badge (matches the Rust `Severity` derive order). */
const SEVERITY_RANK: Record<Severity, number> = { info: 0, warn: 1, critical: 2 }

/** Dedup key for the terrain-drape condition: one row however many different
 * errors the drape reports. */
export const DRAPE_ALERT_KEY = 'drape'

/** The derived values every view needs, recomputed on each transition. */
interface Derived {
  /** Alerts still asking for the operator, newest first. */
  visible: Alert[]
  /** `visible.length`. */
  count: number
  /** Worst severity among `visible` (`null` when nothing is open). */
  highest: Severity | null
  /** Newest unacknowledged Critical, for the persistent banner. */
  banner: Alert | null
}

function sortNewestFirst(alerts: Alert[]): Alert[] {
  return [...alerts].sort((a, b) => b.last_ts_ms - a.last_ts_ms || a.id.localeCompare(b.id))
}

function derive(alerts: Alert[]): Derived {
  const visible = alerts.filter((a) => !a.acked)
  let highest: Severity | null = null
  for (const a of visible) {
    if (highest === null || SEVERITY_RANK[a.severity] > SEVERITY_RANK[highest]) {
      highest = a.severity
    }
  }
  return {
    visible,
    count: visible.length,
    highest,
    banner: visible.find((a) => a.severity === 'critical') ?? null,
  }
}

interface AlertsState extends Derived {
  /** Every retained alert, newest first (history for the drawer). */
  alerts: Alert[]
  /** Replace the whole list — boot, reconnect, or a lagged event stream. */
  setAll: (alerts: Alert[]) => void
  /** Apply one transition from Rust. */
  apply: (event: AlertEvent) => void
  /** Operator acknowledgement. Rust echoes an `alert` event back. */
  ack: (id: string) => void
  /** Acknowledge everything visible. */
  ackAll: () => void
  /** Drop acknowledged alerts from the history. */
  clearAcknowledged: () => void
}

function withAlerts(alerts: Alert[]): Derived & { alerts: Alert[] } {
  return { alerts, ...derive(alerts) }
}

export const useAlerts = create<AlertsState>((set, get) => ({
  ...withAlerts([]),
  setAll: (alerts) => set(withAlerts(sortNewestFirst(alerts))),
  apply: (event) => {
    const rest = get().alerts.filter((a) => a.id !== event.alert.id)
    set(withAlerts(sortNewestFirst([event.alert, ...rest])))
  },
  ack: (id) => {
    void invoke('alert_ack', { id }).catch(() => {})
  },
  ackAll: () => {
    void invoke('alert_ack_all').catch(() => {})
  },
  clearAcknowledged: () => {
    // History-only action: drop the acknowledged rows here and let Rust drop
    // its copies too (the open alerts are never touched).
    set(withAlerts(get().alerts.filter((a) => !a.acked)))
    void invoke('alert_clear_acknowledged').catch(() => {})
  },
}))

/** Load the retained alerts (boot, or after the event stream lagged). */
export async function loadAlerts(): Promise<void> {
  const alerts = await invoke<Alert[]>('alert_snapshot')
  useAlerts.getState().setAll(alerts)
}

function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/** Event-log level for one FC status severity (mirrors the desktop bridge). */
function statusLevel(severity: StatusText['severity']): EventLevel {
  switch (severity) {
    case 'emergency':
    case 'alert':
    case 'critical':
    case 'error':
      return 'err'
    case 'warning':
      return 'warn'
    default:
      return 'info'
  }
}

/**
 * The alert center's one event stream (A3/U3), shared by every window: mount
 * it from the alert layer and the alert model, the badge and the FC status
 * console all follow, in the main window and in the Inspector / Actuator /
 * Params windows alike. Subscribe per mount (not guarded): the transitions are
 * idempotent, so a React StrictMode double-mount is harmless.
 */
export function useAlertStream(): void {
  useEffect(() => {
    if (!inTauri()) return
    let disposed = false
    const unlisteners: Array<() => void> = []
    async function setup() {
      const unAlert = await listen<AlertEvent>('alert', (e) => {
        useAlerts.getState().apply(e.payload)
      })
      if (disposed) return unAlert()
      unlisteners.push(unAlert)

      // L1: every assembled STATUSTEXT is the FC's own log line. A rejected
      // command's reason carries the command name.
      const unText = await listen<StatusText>('statustext', (e) => {
        const { command, text, severity } = e.payload
        logEvent('fc', statusLevel(severity), command ? `${command} rejected: ${text}` : text)
      })
      if (disposed) return unText()
      unlisteners.push(unText)

      // Alerts raised before this window booted (the badge must survive a
      // window reload).
      await loadAlerts().catch(() => {})
    }
    void setup()
    return () => {
      disposed = true
      unlisteners.forEach((u) => u())
    }
  }, [])
}

/**
 * Raise an alert from the frontend: for failures only the UI can see (a DEM
 * the backend cannot drape against, a `.plan` that will not parse). Rust-side
 * sources must call the core center directly instead.
 *
 * Not optimistic — Rust owns the model and echoes the transition back on the
 * `alert` event, so every window sees the same row.
 */
export function raiseAlert(input: {
  source: AlertSource
  severity: Severity
  text: string
  key?: string
}): void {
  void invoke('alert_raise', { input: { ...input, key: input.key ?? null } }).catch(() => {})
}

/**
 * The condition behind a UI-raised alert is gone (a later drape succeeded):
 * acknowledge it without waiting for the operator, because there is nothing
 * left to fix. The row stays in the drawer's history.
 */
export function clearAlert(source: AlertSource, key: string): void {
  void invoke('alert_ack', { id: `${source}|${key}` }).catch(() => {})
}
