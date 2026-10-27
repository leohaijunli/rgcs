// Small Parameters consumer of the native target/client contract (AF-02b.3).
// Requests echo the clicked view's session; none resolves a later global target.
import { invoke } from '@tauri-apps/api/core'
import type { RuntimeClientSession } from '../generated-types/RuntimeClientSession'
import type { RuntimeTargetRef } from '../generated-types/RuntimeTargetRef'
import type { RuntimeTargetSnapshot } from '../generated-types/RuntimeTargetSnapshot'
import type { ParameterRequestContext } from '../generated-types/ParameterRequestContext'

export function sameTarget(a: RuntimeTargetRef, b: RuntimeTargetRef): boolean {
  return a.vehicle_id === b.vehicle_id && a.session_id === b.session_id
    && a.link_id === b.link_id && a.generation === b.generation
    && a.mavlink_target.system_id === b.mavlink_target.system_id
    && a.mavlink_target.component_id === b.mavlink_target.component_id
}

export function sameSession(a: RuntimeClientSession | null, b: RuntimeClientSession | null): boolean {
  return a !== null && b !== null && a.owner.client_id === b.owner.client_id
    && a.owner.instance_id === b.owner.instance_id && sameTarget(a.target, b.target)
}

export function parameterContext(session: RuntimeClientSession): ParameterRequestContext {
  return { session, request_id: crypto.randomUUID() }
}

export async function primaryTarget(): Promise<RuntimeTargetRef | null> {
  const targets = await invoke<RuntimeTargetSnapshot[]>('runtime_targets')
  if (!Array.isArray(targets)) throw new Error('The native Runtime target API is unavailable')
  return targets.find((target) => target.roles.includes('primary') && target.availability === 'available')?.target ?? null
}

export async function attachParameters(target: RuntimeTargetRef): Promise<RuntimeClientSession> {
  return invoke<RuntimeClientSession>('runtime_client_attach', { moduleId: 'parameters', target })
}

export async function detachParameters(session: RuntimeClientSession): Promise<void> {
  await invoke('runtime_client_detach', { session })
}

export function parameterError(error: unknown): string {
  if (typeof error === 'string') return error
  if (typeof error !== 'object' || error === null) return String(error)
  const value = error as { code?: string; message?: string; reason?: string; error?: unknown }
  if (value.reason) return value.reason
  if (value.message) return value.message
  if (value.error) return parameterError(value.error)
  const labels: Record<string, string> = {
    busy: 'Another parameter transaction is active',
    resource_busy: 'The target resource is owned by another operation',
    not_owner: 'This client no longer owns that operation',
    context_mismatch: 'The client or target operation context changed',
    target_changed: 'The target or transport generation changed',
    target_unavailable: 'The target is unavailable or unverified',
    cancelled: 'The parameter operation was cancelled',
    timeout: 'The parameter operation timed out; write results require read-back',
    deadline_exceeded: 'The parameter operation deadline expired',
    not_connected: 'The parameter target disconnected',
    capacity_exceeded: 'The Runtime operation budget is full',
  }
  return (value.code && labels[value.code]) || JSON.stringify(error)
}
