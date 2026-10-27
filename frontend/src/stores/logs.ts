// Target/owner-bound FC logs. Local file selection precedes Runtime admission.
import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { save } from '@tauri-apps/plugin-dialog'
import type { LogEntryInfo } from '../generated-types/LogEntryInfo'
import type { LogReply } from '../generated-types/LogReply'
import type { LogRequestContext } from '../generated-types/LogRequestContext'
import type { LogProgressEnvelope } from '../generated-types/LogProgressEnvelope'
import type { OperationRef } from '../generated-types/OperationRef'
import type { OperationStatus } from '../generated-types/OperationStatus'
import type { RuntimeTargetRef as TargetRef } from '../generated-types/RuntimeTargetRef'
import { isCurrentLogsSession, logsContext, logsFailure, resetLogsRuntime } from '../desktop/logsRuntime'
import { operationMatchesSession, sameRuntimeTarget } from '../desktop/flightRuntime'

export interface LogsDownload { id: number; fetched: number; total: number; path?: string }
interface LogsState {
  entries: LogEntryInfo[]
  downloads: Map<number, LogsDownload>
  busy: boolean
  operation: OperationRef | null
  error: string | null
  reconciliation: boolean
  requestList: () => Promise<void>
  download: (id: number, size: number) => Promise<void>
  cancel: () => Promise<void>
  eraseAll: () => Promise<void>
  remove: (id: number, size: number, timeUtc: number) => Promise<void>
  reset: () => void
}
interface Active { epoch: number; context: LogRequestContext; operation: OperationRef | null }
let epoch = 0
let active: Active | null = null
let listedTarget: TargetRef | null = null
function current(work: Active): boolean {
  return active === work && epoch === work.epoch && isCurrentLogsSession(work.context.session)
}
function acceptOperation(work: Active, operation: OperationRef): boolean {
  return current(work) && operationMatchesSession(operation, work.context.session)
    && (!work.operation || work.operation.id === operation.id)
}
function reset(): void {
  epoch += 1
  active = null
  listedTarget = null
  resetLogsRuntime()
  useLogsStore.setState({ entries: [], downloads: new Map(), busy: false, operation: null, error: null, reconciliation: false })
}
async function run(command: string, input: Record<string, unknown> = {}, expectedTarget: TargetRef | null = null): Promise<void> {
  if (useLogsStore.getState().busy) return
  const requestedEpoch = epoch
  useLogsStore.setState({ busy: true, error: null })
  let work: Active | null = null
  try {
    const context = await logsContext()
    if (epoch !== requestedEpoch) return
    if (expectedTarget && !sameRuntimeTarget(expectedTarget, context.session.target)) {
      listedTarget = null
      useLogsStore.setState({ entries: [], downloads: new Map() })
      throw new Error('The log target changed; refresh before using an old entry')
    }
    work = { epoch, context, operation: null }
    active = work
    const request = command === 'logs_request_list' ? context : { context, ...input }
    const reply = await invoke<LogReply>(command, { request })
    if (!current(work)) return
    if (!acceptOperation(work, reply.operation)) throw new Error('Log reply does not match its owned operation')
    const expected = { logs_request_list: 'listed', logs_download: 'downloaded', logs_delete: 'deleted', logs_erase_all: 'erased' }[command]
    if (reply.result.kind !== expected) throw new Error('Unexpected log operation result')
    const result = reply.result
    if (result.kind === 'listed' || result.kind === 'deleted') {
      listedTarget = context.session.target
      useLogsStore.setState({ entries: result.entries, downloads: new Map(), reconciliation: false })
    } else if (result.kind === 'erased') {
      listedTarget = context.session.target
      useLogsStore.setState({ entries: [], downloads: new Map(), reconciliation: false })
    } else {
      const entry = input.entry as LogEntryInfo
      if (result.entry.id !== entry.id || result.entry.size !== entry.size || result.entry.time_utc !== entry.time_utc
        || result.path !== input.save_path) throw new Error('Downloaded file identity does not match its request')
      const downloads = new Map(useLogsStore.getState().downloads)
      downloads.set(entry.id, { id: entry.id, fetched: entry.size, total: entry.size, path: result.path })
      useLogsStore.setState({ downloads })
    }
    active = null
    useLogsStore.setState({ busy: false, operation: null })
  } catch (error) {
    if (epoch !== requestedEpoch || work && !current(work)) return
    const failure = logsFailure(error)
    active = null
    if (failure.reconciliation) listedTarget = null
    useLogsStore.setState({ busy: false, operation: null, error: failure.message, reconciliation: failure.reconciliation,
      ...(failure.reconciliation ? { entries: [], downloads: new Map() } : {}) })
  }
}
export const useLogsStore = create<LogsState>((set, get) => ({
  entries: [], downloads: new Map(), busy: false, operation: null, error: null, reconciliation: false,
  requestList: () => run('logs_request_list'),
  download: async (id, size) => {
    if (get().busy) return
    const entry = get().entries.find((entry) => entry.id === id && entry.size === size)
    const target = listedTarget
    if (!entry || !target) { set({ error: 'Refresh the target log list before downloading' }); return }
    const requestedEpoch = epoch
    set({ busy: true, error: null })
    try {
      const path = await save({ defaultPath: `log${id}.ulg`, filters: [{ name: 'ULog', extensions: ['ulg'] }] })
      if (epoch !== requestedEpoch) return
      set({ busy: false })
      if (path === null) return // No attach, operation, protocol or file write.
      await run('logs_download', { entry: { ...entry }, save_path: path }, target)
    } catch (error) {
      if (epoch === requestedEpoch) set({ busy: false, error: logsFailure(error).message })
    }
  },
  cancel: async () => {
    const work = active
    if (!work?.operation) { reset(); return } // Retire only this module's prepared session/picker.
    try { await invoke<OperationStatus>('logs_cancel', { session: work.context.session, operation: work.operation }) }
    catch (error) { if (current(work)) set({ error: logsFailure(error).message }) }
  },
  eraseAll: () => {
    if (get().reconciliation || !listedTarget) { set({ error: 'Refresh before deleting all logs' }); return Promise.resolve() }
    return run('logs_erase_all', { confirmation: 'DELETE ALL LOGS' }, listedTarget)
  },
  remove: (id, size, timeUtc) => {
    const entry = get().entries.find((entry) => entry.id === id && entry.size === size && entry.time_utc === timeUtc)
    if (!entry || !listedTarget || get().reconciliation) { set({ error: 'Refresh before deleting an old log entry' }); return Promise.resolve() }
    return run('logs_delete', { entry: { ...entry }, confirmation: `DELETE LOG ${id}` }, listedTarget)
  },
  reset,
}))
let watcherStarted = false
export function startLogsWatcher(): void {
  if (watcherStarted) return
  watcherStarted = true
  void listen<OperationStatus>('logs_operation', ({ payload }) => {
    const work = active
    if (!work || payload.request_id !== work.context.request_id || !acceptOperation(work, payload.operation)) return
    work.operation = payload.operation
    useLogsStore.setState({ operation: payload.operation })
    if (payload.phase === 'finished' && payload.outcome?.kind !== 'succeeded') {
      const uncertain = payload.cleanup === 'uncertain' || payload.outcome?.kind === 'uncertain'
      if (uncertain) listedTarget = null
      useLogsStore.setState({ busy: false, reconciliation: uncertain,
        error: uncertain ? 'Log state is unconfirmed; reconnect and refresh before another destructive action' : 'The log operation stopped',
        ...(uncertain ? { entries: [], downloads: new Map() } : {}) })
    }
  }).catch(() => { watcherStarted = false })
  void listen<LogProgressEnvelope>('logs_progress', ({ payload }) => {
    const work = active
    if (!work || payload.request_id !== work.context.request_id || !acceptOperation(work, payload.operation)) return
    const entry = useLogsStore.getState().entries.find((entry) => entry.id === payload.entry.id
      && entry.size === payload.entry.size && entry.time_utc === payload.entry.time_utc)
    if (!entry || payload.total !== entry.size || payload.fetched > payload.total) return
    work.operation = payload.operation
    const downloads = new Map(useLogsStore.getState().downloads)
    downloads.set(entry.id, { id: entry.id, fetched: payload.fetched, total: payload.total })
    useLogsStore.setState({ downloads, operation: payload.operation })
  }).catch(() => {})
}
