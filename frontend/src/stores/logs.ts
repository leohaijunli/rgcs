// Logs store (issues O1 / operator request): the FC's SD log list, one
// download at a time with progress, fed by the `logs_event` shell events.

import { create } from 'zustand'
import type { LogEntryInfo } from '../generated-types/LogEntryInfo'

export interface LogsDownload {
  id: number
  fetched: number
  total: number
  /** Set when the download finished and the file was written. */
  path?: string
}

interface LogsState {
  entries: LogEntryInfo[]
  /** Per-log download progress (id → state). */
  downloads: Map<number, LogsDownload>
  busy: boolean
  error: string | null
  handleEvent: (e: LogsEventPayload) => void
  requestList: () => Promise<void>
  download: (id: number, size: number) => Promise<void>
  cancel: () => Promise<void>
}

export interface LogsEventPayload {
  kind: 'list' | 'progress' | 'downloaded' | 'failed'
  entries?: LogEntryInfo[]
  id?: number
  fetched?: number
  total?: number
  path?: string
  message?: string
}

export const useLogsStore = create<LogsState>((set, get) => ({
  entries: [],
  downloads: new Map(),
  busy: false,
  error: null,

  handleEvent: (e) => {
    set((s) => {
      switch (e.kind) {
        case 'list':
          return {
            entries: e.entries ?? [],
            busy: false,
            error: null,
            downloads: new Map(),
          }
        case 'progress': {
          const downloads = new Map(s.downloads)
          downloads.set(e.id ?? 0, {
            id: e.id ?? 0,
            fetched: e.fetched ?? 0,
            total: e.total ?? 0,
          })
          return { downloads }
        }
        case 'downloaded': {
          const downloads = new Map(s.downloads)
          const existing = downloads.get(e.id ?? 0)
          if (existing) downloads.set(e.id ?? 0, { ...existing, path: e.path })
          return { downloads, busy: false }
        }
        case 'failed':
          return { busy: false, error: e.message ?? 'failed' }
        default:
          return s
      }
    })
  },

  requestList: async () => {
    set({ busy: true, error: null })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('logs_request_list')
    } catch (err) {
      set({ busy: false, error: String(err) })
    }
  },

  download: async (id, size) => {
    set({ busy: true, error: null })
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('logs_download', { id, size })
      // The dialog may have been cancelled; the busy flag clears on the
      // next terminal event anyway.
    } catch (err) {
      set({ busy: false, error: String(err) })
    }
    void get()
  },

  cancel: async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      await invoke('logs_cancel')
    } catch {
      /* not connected */
    }
    set({ busy: false })
  },
}))

let watcherStarted = false
/** Subscribe once (desktop only) at Logs tab mount. */
export function startLogsWatcher(): void {
  if (watcherStarted) return
  watcherStarted = true
  void (async () => {
    try {
      const { listen } = await import('@tauri-apps/api/event')
      await listen<LogsEventPayload>('logs_event', (e) =>
        useLogsStore.getState().handleEvent(e.payload),
      )
    } catch {
      /* browser dev: no shell events */
    }
  })()
}
