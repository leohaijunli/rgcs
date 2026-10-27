import { invoke } from '@tauri-apps/api/core'
import type { MotorRequestContext } from '../generated-types/MotorRequestContext'
import type { RuntimeClientSession } from '../generated-types/RuntimeClientSession'
import type { RuntimeTargetSnapshot } from '../generated-types/RuntimeTargetSnapshot'
import { sameRuntimeSession, sameRuntimeTarget } from '../desktop/flightRuntime'

const MODULE_ID = 'motor-test'
let session: RuntimeClientSession | null = null
let epoch = 0
let queue: Promise<void> = Promise.resolve()
function serialize<T>(work: () => Promise<T>): Promise<T> {
  const result = queue.then(work, work)
  queue = result.then(() => undefined, () => undefined)
  return result
}
async function detach(stale: RuntimeClientSession): Promise<void> {
  try { await invoke('runtime_client_detach', { session: stale }) } catch { /* native close/target retirement may win */ }
}
export function currentMotorSession(): RuntimeClientSession | null { return session }
export function isCurrentMotorSession(expected: RuntimeClientSession): boolean { return sameRuntimeSession(session, expected) }
export function resetMotorRuntime(): void {
  epoch += 1
  const stale = session
  session = null
  if (stale) void serialize(() => detach(stale))
}
export async function motorContext(): Promise<MotorRequestContext> {
  const requestedEpoch = epoch
  const attached = await serialize(async () => {
    if (requestedEpoch !== epoch) throw new Error('The Motor Test target changed')
    const snapshots = await invoke<RuntimeTargetSnapshot[]>('runtime_targets')
    const target = Array.isArray(snapshots) ? snapshots.find((snapshot) => snapshot.roles.includes('motor_test')
      && snapshot.availability === 'available' && snapshot.target !== null)?.target ?? null : null
    if (!target) throw new Error('No available Motor Test target')
    const previous = session
    if (previous && sameRuntimeTarget(previous.target, target)) {
      try {
        await invoke('runtime_client_validate', { session: previous })
        if (requestedEpoch !== epoch || !sameRuntimeSession(session, previous)) throw new Error('The Motor Test target changed')
        return previous
      } catch (error) {
        if (requestedEpoch !== epoch) throw error
        session = null
        await detach(previous)
      }
    } else if (previous) { session = null; await detach(previous) }
    const next = await invoke<RuntimeClientSession>('runtime_client_attach', { moduleId: MODULE_ID, target })
    if (requestedEpoch !== epoch || next.module_id !== MODULE_ID || !sameRuntimeTarget(next.target, target)) {
      await detach(next)
      throw new Error('The Motor Test target changed while attaching')
    }
    session = next
    return next
  })
  return { session: attached, request_id: crypto.randomUUID() }
}
