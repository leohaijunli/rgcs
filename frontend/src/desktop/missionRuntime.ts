import { invoke } from '@tauri-apps/api/core'
import type { MissionRequestContext } from '../generated-types/MissionRequestContext'
import type { RuntimeClientSession } from '../generated-types/RuntimeClientSession'
import type { RuntimeTargetSnapshot } from '../generated-types/RuntimeTargetSnapshot'
import { sameRuntimeSession, sameRuntimeTarget } from './flightRuntime'

const MODULE_ID = 'mission'
let session: RuntimeClientSession | null = null
let epoch = 0
let queue: Promise<void> = Promise.resolve()

function serialize<T>(work: () => Promise<T>): Promise<T> {
  const result = queue.then(work, work)
  queue = result.then(() => undefined, () => undefined)
  return result
}

async function detach(stale: RuntimeClientSession): Promise<void> {
  try { await invoke('runtime_client_detach', { session: stale }) } catch { /* Native retirement may already have won. */ }
}

export function isCurrentMissionSession(expected: RuntimeClientSession): boolean {
  return sameRuntimeSession(session, expected)
}

export function resetMissionRuntime(): void {
  epoch += 1
  const stale = session
  session = null
  if (stale) void serialize(() => detach(stale))
}

export async function missionContext(): Promise<MissionRequestContext> {
  const requestedEpoch = epoch
  const attached = await serialize(async () => {
    if (requestedEpoch !== epoch) throw new Error('The mission target changed')
    const snapshots = await invoke<RuntimeTargetSnapshot[]>('runtime_targets')
    if (!Array.isArray(snapshots)) throw new Error('The native Runtime target API is unavailable')
    if (requestedEpoch !== epoch) throw new Error('The mission target changed')
    const target = snapshots.find((snapshot) => snapshot.roles.includes('primary')
      && snapshot.availability === 'available' && snapshot.target !== null)?.target ?? null
    if (!target) {
      const stale = session
      session = null
      if (stale) await detach(stale)
      throw new Error('No available primary mission target')
    }
    const previous = session
    if (previous && sameRuntimeTarget(previous.target, target)) {
      try {
        await invoke('runtime_client_validate', { session: previous })
        if (requestedEpoch !== epoch || !sameRuntimeSession(session, previous)) throw new Error('The mission target changed')
        return previous
      } catch (error) {
        if (requestedEpoch !== epoch) throw error
        session = null
        await detach(previous)
      }
    } else if (previous) {
      session = null
      await detach(previous)
    }
    const next = await invoke<RuntimeClientSession>('runtime_client_attach', { moduleId: MODULE_ID, target })
    if (requestedEpoch !== epoch || next.module_id !== MODULE_ID || !sameRuntimeTarget(next.target, target)) {
      await detach(next)
      throw new Error('The mission target changed while attaching')
    }
    session = next
    return next
  })
  return { session: attached, request_id: crypto.randomUUID() }
}

export interface MissionFailure { code: string | null; message: string; reconciliation: boolean }

export function missionFailure(error: unknown): MissionFailure {
  let value: unknown = error instanceof Error ? { message: error.message } : error
  if (typeof value === 'string') {
    try { value = JSON.parse(value) as unknown } catch { return { code: null, message: value as string, reconciliation: false } }
  }
  const decoded: { code: string | null; message: string | null; acknowledgement: number | null; reconciliation: boolean } = {
    code: null, message: null, acknowledgement: null, reconciliation: false,
  }
  const walk = (value: unknown, depth = 0): void => {
    if (depth > 8 || typeof value !== 'object' || value === null || Array.isArray(value)) return
    const record = value as Record<string, unknown>
    if (typeof record.code === 'string') {
      decoded.code = record.code
      if (decoded.code === 'reconciliation_required' || decoded.code === 'ambiguous_reply') decoded.reconciliation = true
    }
    if (decoded.message === null && typeof record.reason === 'string') decoded.message = record.reason
    if (decoded.message === null && typeof record.message === 'string') decoded.message = record.message
    if (typeof record.ack_type === 'number') decoded.acknowledgement = record.ack_type
    if (record.error !== undefined) walk(record.error, depth + 1)
  }
  walk(value)
  const labels: Record<string, string> = {
    resource_busy: 'Another operation owns the mission resource', busy: 'Another mission exchange is active',
    cancelled: 'The mission operation was cancelled', target_changed: 'The mission target changed',
    target_unavailable: 'The mission target is unavailable', not_owner: 'The mission session no longer owns this operation',
    context_mismatch: 'The mission session or target changed', stopped: 'The mission service stopped',
    timed_out: 'The mission deadline expired', deadline_exceeded: 'The mission deadline expired',
    retries_exhausted: 'The FC mission exchange exhausted its retry budget',
    ambiguous_reply: 'Mission reply authority is uncertain; reconnect and read back the FC mission before retrying',
    capacity_exceeded: 'The Runtime operation budget is full', no_items: 'No mission items to upload',
  }
  if (decoded.code === 'ack_denied') decoded.message = `The flight controller rejected the mission (type ${decoded.acknowledgement ?? 'unknown'})`
  return { code: decoded.code, message: decoded.message ?? (decoded.code ? labels[decoded.code] ?? decoded.code.replaceAll('_', ' ') : String(error)), reconciliation: decoded.reconciliation }
}
