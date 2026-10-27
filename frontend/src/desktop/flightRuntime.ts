import { invoke } from '@tauri-apps/api/core'
import type { CommandRequestContext } from '../generated-types/CommandRequestContext'
import type { FlightCommandError } from '../generated-types/FlightCommandError'
import type { OperationRef } from '../generated-types/OperationRef'
import type { PreflightRefusal } from '../generated-types/PreflightRefusal'
import type { RuntimeClientSession } from '../generated-types/RuntimeClientSession'
import type { RuntimeTargetRef } from '../generated-types/RuntimeTargetRef'
import type { RuntimeTargetSnapshot } from '../generated-types/RuntimeTargetSnapshot'

const FLIGHT_MODULE_ID = 'flight-controls'

let currentSession: RuntimeClientSession | null = null
let scopeEpoch = 0
let sessionQueue: Promise<void> = Promise.resolve()

export interface FlightCommandFailure {
  message: string
  preflight: PreflightRefusal | null
  result: string | null
}

export function sameRuntimeTarget(a: RuntimeTargetRef, b: RuntimeTargetRef): boolean {
  return a.vehicle_id === b.vehicle_id
    && a.session_id === b.session_id
    && a.link_id === b.link_id
    && a.generation === b.generation
    && a.mavlink_target.system_id === b.mavlink_target.system_id
    && a.mavlink_target.component_id === b.mavlink_target.component_id
}

export function sameRuntimeSession(
  a: RuntimeClientSession | null,
  b: RuntimeClientSession | null,
): boolean {
  return a !== null
    && b !== null
    && a.module_id === b.module_id
    && a.owner.client_id === b.owner.client_id
    && a.owner.instance_id === b.owner.instance_id
    && sameRuntimeTarget(a.target, b.target)
}

export function operationMatchesSession(
  operation: OperationRef,
  session: RuntimeClientSession,
): boolean {
  return operation.owner.client_id === session.owner.client_id
    && operation.owner.instance_id === session.owner.instance_id
    && sameRuntimeTarget(operation.target, session.target)
}

export function isCurrentFlightRuntimeSession(session: RuntimeClientSession): boolean {
  return sameRuntimeSession(currentSession, session)
}

/** Bind the current available primary target and create a one-use request identity. */
export async function flightCommandContext(): Promise<CommandRequestContext> {
  const epoch = scopeEpoch
  const session = await serializeSession(async () => {
    if (epoch !== scopeEpoch) throw new Error('The flight target changed')

    const targets = await invoke<RuntimeTargetSnapshot[]>('runtime_targets')
    if (!Array.isArray(targets)) throw new Error('The native Runtime target API is unavailable')
    if (epoch !== scopeEpoch) throw new Error('The flight target changed')

    const target = targets.find((candidate) =>
      candidate.roles.includes('primary')
      && candidate.availability === 'available'
      && candidate.target !== null,
    )?.target ?? null
    if (!target) {
      await detachCurrentSession()
      throw new Error('No available primary flight target')
    }

    const existing = currentSession
    if (existing && sameRuntimeTarget(existing.target, target)) {
      try {
        await invoke('runtime_client_validate', { session: existing })
        if (epoch !== scopeEpoch || !sameRuntimeSession(currentSession, existing)) {
          throw new Error('The flight target changed')
        }
        return existing
      } catch (error) {
        if (epoch !== scopeEpoch) throw error
        if (sameRuntimeSession(currentSession, existing)) currentSession = null
        await detachSession(existing)
      }
    } else if (existing) {
      currentSession = null
      await detachSession(existing)
    }

    const attached = await invoke<RuntimeClientSession>('runtime_client_attach', {
      moduleId: FLIGHT_MODULE_ID,
      target,
    })
    if (
      epoch !== scopeEpoch
      || attached.module_id !== FLIGHT_MODULE_ID
      || !sameRuntimeTarget(attached.target, target)
    ) {
      await detachSession(attached)
      throw new Error('The flight target changed while attaching')
    }
    currentSession = attached
    return attached
  })

  return { session, request_id: crypto.randomUUID() }
}

/** Invalidate reply authority immediately, then retire the native attachment. */
export function resetFlightRuntimeSession(): void {
  scopeEpoch += 1
  const stale = currentSession
  currentSession = null
  if (stale) void serializeSession(() => detachSession(stale))
}

/** Decode Tauri's object/string rejection and retain structured preflight data. */
export function flightCommandError(error: unknown): FlightCommandFailure {
  const decoded = decodeError(error)
  const typed = isRecord(decoded) ? decoded as unknown as FlightCommandError : null
  const value: unknown = typed ?? decoded
  const preflight = findPreflight(value)
  const code = findErrorCode(value)
  return {
    preflight,
    result: code === 'superseded' ? 'superseded' : null,
    message: preflight?.message ?? findErrorMessage(value) ?? errorLabel(code) ?? String(error),
  }
}

function serializeSession<T>(work: () => Promise<T>): Promise<T> {
  const result = sessionQueue.then(work, work)
  sessionQueue = result.then(() => undefined, () => undefined)
  return result
}

async function detachCurrentSession(): Promise<void> {
  const stale = currentSession
  currentSession = null
  if (stale) await detachSession(stale)
}

async function detachSession(session: RuntimeClientSession): Promise<void> {
  try {
    await invoke('runtime_client_detach', { session })
  } catch {
    // A replaced target or native window cleanup may already own retirement.
  }
}

function decodeError(error: unknown): unknown {
  if (typeof error === 'string') {
    try {
      return JSON.parse(error) as unknown
    } catch {
      return error
    }
  }
  if (error instanceof Error) {
    try {
      return JSON.parse(error.message) as unknown
    } catch {
      return { message: error.message }
    }
  }
  return error
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function findPreflight(value: unknown, depth = 0): PreflightRefusal | null {
  if (depth > 8 || !isRecord(value)) return null
  if (
    typeof value.kind === 'string'
    && typeof value.message === 'string'
    && Array.isArray(value.items)
  ) {
    return value as unknown as PreflightRefusal
  }
  for (const nested of Object.values(value)) {
    const refusal = findPreflight(nested, depth + 1)
    if (refusal) return refusal
  }
  return null
}

function findErrorCode(value: unknown, depth = 0): string | null {
  if (depth > 8 || !isRecord(value)) return null
  for (const key of ['error', 'cause', 'command']) {
    const nested = findErrorCode(value[key], depth + 1)
    if (nested) return nested
  }
  return typeof value.code === 'string' ? value.code : null
}

function findErrorMessage(value: unknown, depth = 0): string | null {
  if (depth > 8) return null
  if (typeof value === 'string') return value
  if (!isRecord(value)) return null
  if (typeof value.reason === 'string') return value.reason
  if (typeof value.message === 'string') return value.message
  for (const key of ['error', 'cause', 'command']) {
    const nested = findErrorMessage(value[key], depth + 1)
    if (nested) return nested
  }
  return null
}

function errorLabel(code: string | null): string | null {
  if (!code) return null
  const labels: Record<string, string> = {
    busy: 'Another flight command is active',
    resource_busy: 'The flight-command resource is busy',
    no_ack: 'The flight controller did not acknowledge the command',
    link_lost: 'The flight link was lost',
    superseded: 'Superseded by a higher-priority flight command',
    service_stopped: 'The flight-command service stopped',
    timed_out: 'The flight command timed out',
    deadline_exceeded: 'The flight-command deadline expired',
    cancelled: 'The flight command was cancelled',
    target_changed: 'The flight target changed',
    target_unavailable: 'The flight target is unavailable',
    not_owner: 'This flight-control session no longer owns the operation',
    context_mismatch: 'The flight-control target or session changed',
    capacity_exceeded: 'The Runtime operation budget is full',
  }
  return labels[code] ?? code.replaceAll('_', ' ')
}
