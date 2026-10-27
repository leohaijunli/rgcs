import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { OperationRef } from '../generated-types/OperationRef'
import type { OperationStatus } from '../generated-types/OperationStatus'
import type { ShellCommandRequest } from '../generated-types/ShellCommandRequest'
import type { ShellOutputEnvelope } from '../generated-types/ShellOutputEnvelope'
import type { ShellSessionReply } from '../generated-types/ShellSessionReply'
import type { RuntimeClientSession } from '../generated-types/RuntimeClientSession'
import type { RuntimeTargetRef } from '../generated-types/RuntimeTargetRef'
import { consoleContext, consoleError, isCurrentConsoleSession, resetConsoleRuntime } from '../desktop/consoleRuntime'
import { operationMatchesSession, sameRuntimeTarget } from '../desktop/flightRuntime'

const MAX_TRANSCRIPT_CHARS = 64 * 1024
const SHELL_CONFIRMATION = 'RUN'

export const CONSOLE_PRESETS: string[] = [
  'help', 'ver', 'top', 'dmesg', 'free', 'ps', 'listener sensor_combined',
  'listener vehicle_gps_position', 'param show', 'gps status',
]
const READ_ONLY = new Set(['help', 'ver', 'top', 'dmesg', 'free', 'ps', 'param show', 'gps status'])
export function isReadOnlyConsoleCommand(command: string): boolean {
  return READ_ONLY.has(command) || (/^[A-Za-z0-9 _.*:-]+$/.test(command)
    && (command.startsWith('listener ') || command.startsWith('param show ')))
}

interface ConsoleState {
  transcript: string
  busy: boolean
  operation: OperationRef | null
  error: string | null
  outputGap: boolean
  send: (command: string, confirmed?: boolean, reviewedTarget?: RuntimeTargetRef) => Promise<void>
  close: () => Promise<void>
  cancel: () => Promise<void>
  startWatcher: () => void
  reset: () => void
}

let started = false
let shellSession: RuntimeClientSession | null = null
let expectedSequence = 1n
let consoleEpoch = 0

function appendOutput(envelope: ShellOutputEnvelope): void {
  const state = useConsoleStore.getState()
  if (!state.operation || !shellSession || !isCurrentConsoleSession(shellSession)
    || !operationMatchesSession(envelope.operation, shellSession)
    || envelope.operation.id !== state.operation.id
    || !sameRuntimeTarget(envelope.target, shellSession.target)) return
  let sequence: bigint | null = null
  try { sequence = BigInt(envelope.sequence) } catch { /* malformed wire counter */ }
  const gap = sequence === null || sequence !== expectedSequence
  expectedSequence = sequence !== null ? sequence + 1n : expectedSequence + 1n
  const prefix = gap ? '\n[console output gap]\n' : ''
  const combined = `${state.transcript}${prefix}${envelope.chunk}`
  useConsoleStore.setState({ transcript: combined.slice(-MAX_TRANSCRIPT_CHARS), outputGap: state.outputGap || gap })
}

async function ensureConsole(): Promise<{ session: RuntimeClientSession; operation: OperationRef }> {
  const epoch = consoleEpoch
  const current = useConsoleStore.getState()
  if (current.operation && shellSession && isCurrentConsoleSession(shellSession)) {
    await invoke('runtime_client_validate', { session: shellSession })
    if (epoch !== consoleEpoch || !shellSession) throw new Error('The console target changed')
    return { session: shellSession, operation: current.operation }
  }
  const context = await consoleContext()
  if (epoch !== consoleEpoch) throw new Error('The console target changed')
  const reply = await invoke<ShellSessionReply>('console_open', { request: context })
  if (epoch !== consoleEpoch || !isCurrentConsoleSession(context.session)) {
    void invoke('console_cancel', { session: context.session, operation: reply.operation }).catch(() => {})
    throw new Error('The console target changed while opening')
  }
  if (!operationMatchesSession(reply.operation, context.session)) throw new Error('Console open reply target does not match its session')
  shellSession = context.session
  expectedSequence = 1n
  useConsoleStore.setState({ operation: reply.operation, outputGap: false })
  return { session: context.session, operation: reply.operation }
}

function retireConsole(): void {
  consoleEpoch += 1
  shellSession = null
  resetConsoleRuntime()
  expectedSequence = 1n
  useConsoleStore.setState({ busy: false, operation: null, outputGap: false })
}

export const useConsoleStore = create<ConsoleState>((set, get) => ({
  transcript: '', busy: false, operation: null, error: null, outputGap: false,
  send: async (command, confirmed = false, reviewedTarget) => {
    if (!command || get().busy) return
    const epoch = consoleEpoch
    set({ busy: true, error: null })
    try {
      if (reviewedTarget) {
        const context = await consoleContext()
        if (!sameRuntimeTarget(reviewedTarget, context.session.target)) throw new Error('The reviewed console target changed; review the command again')
      }
      const owner = await ensureConsole()
      if (epoch !== consoleEpoch) return
      if (reviewedTarget && !sameRuntimeTarget(reviewedTarget, owner.session.target)) throw new Error('The reviewed console target changed')
      const request: ShellCommandRequest = {
        session: owner.session, operation: owner.operation, request_id: crypto.randomUUID(), command,
        confirmation: confirmed ? SHELL_CONFIRMATION : null,
      }
      await invoke('console_send', { request })
      if (epoch !== consoleEpoch || !isCurrentConsoleSession(owner.session)) return
      set({ busy: false })
    } catch (error) {
      if (epoch !== consoleEpoch) return
      const decoded = consoleError(error)
      set({ busy: false, error: decoded.message })
      if (['target_changed', 'target_unavailable', 'not_owner', 'stopped'].includes(decoded.code ?? '')) retireConsole()
      throw error
    }
  },
  close: async () => {
    const epoch = consoleEpoch
    const operation = get().operation
    const session = shellSession
    if (!operation || !session) { retireConsole(); return }
    set({ busy: true })
    try {
      const status = await invoke<OperationStatus>('console_close', { session, operation })
      if (epoch !== consoleEpoch) return
      if (status.cleanup === 'uncertain') set({ error: 'Console release is unconfirmed; reconnect before another shell/log operation' })
    } catch (error) {
      if (epoch !== consoleEpoch) return
      set({ error: consoleError(error).message })
    }
    retireConsole()
  },
  cancel: async () => {
    const operation = get().operation
    const session = shellSession
    if (!operation || !session) { retireConsole(); return }
    try { await invoke('console_cancel', { session, operation }) }
    catch (error) { set({ error: consoleError(error).message }) }
  },
  startWatcher: () => {
    if (started) return
    started = true
    void listen<ShellOutputEnvelope>('console_output', (event) => appendOutput(event.payload)).catch(() => {})
    void listen<OperationStatus>('console_operation', (event) => {
      const operation = get().operation
      if (!operation || !shellSession || event.payload.operation.id !== operation.id
        || !operationMatchesSession(event.payload.operation, shellSession)) return
      if (event.payload.phase === 'finished') {
        if (event.payload.cleanup === 'uncertain') set({ error: 'Console release is unconfirmed; reconnect before another shell/log operation' })
        retireConsole()
      }
    }).catch(() => {})
  },
  reset: () => retireConsole(),
}))
