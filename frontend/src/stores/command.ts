// Flight command state (issue #5): tracks a command's round-trip through the
// desktop command service, which retransmits a COMMAND_LONG until the flight
// controller acks it. The terminal outcome arrives on the `"command"` event.

import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'

/**
 * Commands the UI may send. Pause/resume are deliberately absent: PX4 v1.17
 * `DO_PAUSE_CONTINUE` semantics are unverified (see issues.md #5).
 */
export type CommandName = 'rtl'

/** Payload of the backend `command` event. */
export interface CommandEventPayload {
  command: string
  kind: 'sent' | 'completed' | 'failed'
  result?: string | null
  message?: string | null
}

interface CommandState {
  /** Command awaiting a terminal ack, or null when idle. */
  pending: CommandName | null
  /** Most recent event, kept for the status line. */
  lastEvent: CommandEventPayload | null
  send: (name: CommandName) => Promise<void>
  handleEvent: (e: CommandEventPayload) => void
  reset: () => void
}

export const useCommandStore = create<CommandState>((set) => ({
  pending: null,
  lastEvent: null,

  send: async (name) => {
    set({ pending: name, lastEvent: null })
    try {
      await invoke('send_command', { name })
    } catch (err) {
      set({
        pending: null,
        lastEvent: {
          command: name,
          kind: 'failed',
          result: null,
          message: String(err),
        },
      })
    }
  },

  // `sent` (including retransmissions) keeps the command pending; a
  // `completed`/`failed` event always ends it.
  handleEvent: (e) =>
    set((s) => ({
      lastEvent: e,
      pending: e.kind === 'sent' ? s.pending : null,
    })),

  reset: () => set({ pending: null, lastEvent: null }),
}))
