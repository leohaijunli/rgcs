import { invoke } from '@tauri-apps/api/core'
import type { LogRequestContext } from '../generated-types/LogRequestContext'
import type { RuntimeClientSession } from '../generated-types/RuntimeClientSession'
import type { RuntimeTargetSnapshot } from '../generated-types/RuntimeTargetSnapshot'
import { sameRuntimeSession, sameRuntimeTarget } from './flightRuntime'
import { consoleError } from './consoleRuntime'

const MODULE_ID = 'logs'
let session: RuntimeClientSession | null = null
let epoch = 0
let queue: Promise<void> = Promise.resolve()
function serialize<T>(work: () => Promise<T>): Promise<T> {
  const result = queue.then(work, work)
  queue = result.then(() => undefined, () => undefined)
  return result
}
async function detach(stale: RuntimeClientSession): Promise<void> {
  try { await invoke('runtime_client_detach', { session: stale }) } catch { /* already retired */ }
}
export function isCurrentLogsSession(expected: RuntimeClientSession): boolean { return sameRuntimeSession(session, expected) }
export function resetLogsRuntime(): void {
  epoch += 1
  const stale = session
  session = null
  if (stale) void serialize(() => detach(stale))
}
export async function logsContext(): Promise<LogRequestContext> {
  const requestedEpoch = epoch
  const attached = await serialize(async () => {
    if (requestedEpoch !== epoch) throw new Error('The log target changed')
    const snapshots = await invoke<RuntimeTargetSnapshot[]>('runtime_targets')
    if (requestedEpoch !== epoch) throw new Error('The log target changed')
    const target = Array.isArray(snapshots) ? snapshots.find((snapshot) => snapshot.roles.includes('primary')
      && snapshot.availability === 'available' && snapshot.target !== null)?.target ?? null : null
    if (!target) throw new Error('No available primary log target')
    const previous = session
    if (previous && sameRuntimeTarget(previous.target, target)) {
      try {
        await invoke('runtime_client_validate', { session: previous })
        if (requestedEpoch !== epoch || !sameRuntimeSession(session, previous)) throw new Error('The log target changed')
        return previous
      } catch (error) {
        if (requestedEpoch !== epoch) throw error
        session = null
        await detach(previous)
      }
    } else if (previous) { session = null; await detach(previous) }
    if (requestedEpoch !== epoch) throw new Error('The log target changed')
    const next = await invoke<RuntimeClientSession>('runtime_client_attach', { moduleId: MODULE_ID, target })
    if (requestedEpoch !== epoch || next.module_id !== MODULE_ID || !sameRuntimeTarget(next.target, target)) {
      await detach(next)
      throw new Error('The log target changed while attaching')
    }
    session = next
    return next
  })
  return { session: attached, request_id: crypto.randomUUID() }
}
export function logsFailure(error: unknown): { message: string; reconciliation: boolean } {
  const decoded = consoleError(error)
  const labels: Record<string, string> = {
    resource_busy: 'Another shell or log operation owns the target', cancelled: 'The log operation was cancelled',
    target_changed: 'The log target changed; refresh its list', timed_out: 'The log operation deadline expired',
    no_ack: 'The FC did not return its log list/data; silence is not an empty SD card',
    cleanup_timeout: 'Log cleanup is unconfirmed; reconnect and refresh before retrying',
  }
  return { message: decoded.code && labels[decoded.code] ? labels[decoded.code] : decoded.message,
    reconciliation: decoded.code === 'reconciliation_required' || decoded.code === 'cleanup_timeout' }
}
