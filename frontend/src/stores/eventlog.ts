// Event log (operator request 2026-10-11): one timestamped stream for
// everything the nav bar used to surface inline — command acks, mode
// switches, arm/disarm, RTK alarms, link changes. In-memory ring; the
// drawer reads it.

import { create } from 'zustand'

export type EventLevel = 'ok' | 'warn' | 'err' | 'info'

export interface LogEvent {
  /** Epoch ms. */
  ts: number
  source: string
  level: EventLevel
  text: string
}

interface EventLogState {
  events: LogEvent[]
  push: (e: Omit<LogEvent, 'ts'>) => void
  clear: () => void
}

/** Keep the newest N; enough history to debug a flight, small enough to be
 * free. */
const MAX_EVENTS = 300

export const useEventLog = create<EventLogState>((set) => ({
  events: [],
  push: (e) =>
    set((s) => ({
      events: [{ ...e, ts: Date.now() }, ...s.events].slice(0, MAX_EVENTS),
    })),
  clear: () => set({ events: [] }),
}))

/** Convenience: push with the timestamp filled in. */
export function logEvent(source: string, level: EventLevel, text: string): void {
  useEventLog.getState().push({ source, level, text })
}
