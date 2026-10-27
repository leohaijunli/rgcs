import { invoke } from '@tauri-apps/api/core'
import type { ShellRequestContext } from '../generated-types/ShellRequestContext'
import type { RuntimeClientSession } from '../generated-types/RuntimeClientSession'
import type { RuntimeTargetSnapshot } from '../generated-types/RuntimeTargetSnapshot'
import { sameRuntimeSession, sameRuntimeTarget } from './flightRuntime'

const MODULE_ID = 'console'
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

export function currentConsoleSession(): RuntimeClientSession | null { return session }
export function isCurrentConsoleSession(expected: RuntimeClientSession): boolean { return sameRuntimeSession(session, expected) }

export function resetConsoleRuntime(): void {
  epoch += 1
  const stale = session
  session = null
  if (stale) void serialize(() => detach(stale))
}

export async function consoleContext(): Promise<ShellRequestContext> {
  const requestedEpoch = epoch
  const attached = await serialize(async () => {
    if (requestedEpoch !== epoch) throw new Error('The console target changed')
    const targets = await invoke<RuntimeTargetSnapshot[]>('runtime_targets')
    if (requestedEpoch !== epoch) throw new Error('The console target changed')
    const target = Array.isArray(targets) ? targets.find((snapshot) => snapshot.roles.includes('primary')
      && snapshot.availability === 'available' && snapshot.target !== null)?.target ?? null : null
    if (!target) throw new Error('No available primary console target')
    const previous = session
    if (previous && sameRuntimeTarget(previous.target, target)) {
      try {
        await invoke('runtime_client_validate', { session: previous })
        if (requestedEpoch !== epoch || !sameRuntimeSession(session, previous)) throw new Error('The console target changed')
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
      throw new Error('The console target changed while attaching')
    }
    session = next
    return next
  })
  return { session: attached, request_id: crypto.randomUUID() }
}

export function consoleError(error: unknown): { code: string | null; message: string } {
  let value: unknown = error instanceof Error ? { message: error.message } : error
  if (typeof value === 'string') {
    const text = value
    try { value = JSON.parse(text) as unknown } catch { return { code: null, message: text } }
  }
  const result: { code: string | null; message: string | null } = { code: null, message: null }
  const visit = (candidate: unknown, depth = 0): void => {
    if (depth > 8 || typeof candidate !== 'object' || candidate === null || Array.isArray(candidate)) return
    const record = candidate as Record<string, unknown>
    if (typeof record.code === 'string') result.code = record.code
    if (result.message === null && typeof record.reason === 'string') result.message = record.reason
    if (result.message === null && typeof record.message === 'string') result.message = record.message
    if (record.error !== undefined) visit(record.error, depth + 1)
  }
  visit(value)
  const labels: Record<string, string> = {
    confirmation_required: 'Type RUN to send this free-form shell command',
    resource_busy: 'The target shell or log resource is busy', busy: 'The target shell is busy',
    target_changed: 'The console target changed', target_unavailable: 'The console target is unavailable',
    cancelled: 'The console session was cancelled', deadline_exceeded: 'The console session expired',
    not_owner: 'This window no longer owns the console session', stopped: 'The console service stopped',
  }
  return { code: result.code, message: result.message ?? (result.code ? labels[result.code] ?? result.code.replaceAll('_', ' ') : String(error)) }
}
