// Readable motor-operation failures over the native IPC.
//
// `actuator_*` commands reject with a serialized Rust enum (`MotorOperationError`
// / `LogOperationError`), not a string: `String(error)` therefore renders
// "[object Object]" in the bench header. Every message below mirrors the
// `thiserror` Display text in core so the operator sees the same reason the
// native side recorded.

const OPERATION: Record<string, string> = {
  not_owner: 'the client/module does not own this operation',
  not_found: 'the operation is not retained',
  context_mismatch: 'operation context does not match its original owner/target',
  capacity_exceeded: 'the operation registry is at its configured capacity',
  duplicate_request: 'this request was already recorded; intent was not replayed',
  cancelled: 'operation cancellation was requested',
  superseded: 'superseded by an admitted emergency command',
  deadline_exceeded: 'the operation deadline expired',
  target_changed: "the operation's bound target expired",
  already_finished: 'the operation has already finished',
  registry_unavailable: 'the operation registry is unavailable',
}

const RUNTIME: Record<string, string> = {
  target_unavailable: 'the requested target is unavailable or unverified',
  identity_exhausted: 'the identity space is exhausted',
}

const MOTOR_PROTOCOL: Record<string, string> = {
  disabled: 'motor control is OFF',
  permission_changed: 'the native motor permission/configuration changed',
  service_stopped: 'the motor service stopped',
  timed_out: 'the motor operation deadline expired',
}

const CAPTURE: Record<string, string> = {
  target_unavailable: 'the requested target is unavailable or unverified',
  target_changed: 'the target session, route or transport generation changed',
  logger_not_running: 'the logger is not running',
  logger_file_backend_unavailable: 'file-backed logging is unavailable',
  start_not_confirmed: 'the capture start was not confirmed',
  stop_not_confirmed: 'the capture stop was not confirmed',
  invalid_test_id: 'the test id is invalid',
  busy: 'another capture exchange is active',
}

const LOG_PROTOCOL: Record<string, string> = {
  stopped: 'log service stopped',
  busy: 'another log exchange is active',
  cancelled: 'the owned log operation was cancelled',
  target_changed: 'the log operation belongs to an earlier target generation',
  timed_out: 'the log operation deadline expired',
  already_submitted: 'this log transaction was already submitted',
  cleanup_timeout: 'log protocol cleanup did not reach a quiet receipt',
}

/** Wrapper variants whose real reason is one level down. */
const WRAPPERS = new Set(['operation', 'protocol', 'runtime', 'capture'])

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function field(value: Record<string, unknown>, key: string): string | null {
  const entry = value[key]
  return typeof entry === 'string' && entry.length > 0 ? entry : null
}

function humanize(code: string): string {
  return code.replaceAll('_', ' ')
}

function describe(value: Record<string, unknown>, depth: number): string {
  const code = field(value, 'code') ?? field(value, 'kind')
  // Nested reason variants: {code: "operation", error: {...}}.
  const nested = value.error
  const detail = field(value, 'message') ?? field(value, 'reason') ?? field(value, 'detail')
  if (code === null) {
    if (isRecord(nested) && depth < 4) return describe(nested, depth + 1)
    return detail ?? JSON.stringify(value)
  }
  if (WRAPPERS.has(code) && isRecord(nested) && depth < 4) {
    return describe(nested, depth + 1)
  }
  // The outer and inner variants both serialize `target_changed`; prefer the
  // inner, more specific "session/route/generation" message.
  if (depth > 0 && code === 'target_changed') {
    return 'the target session, route or transport generation changed'
  }
  switch (code) {
    case 'reconciliation_required':
      return `the operation requires reconciliation: ${detail ?? 'reason unknown'}`
    case 'resource_busy':
      return 'another motor operation still owns the actuator output — wait for it to stop, then retry'
    case 'release_write':
      return `motor ${String(value.motor ?? '?')} release write failed: ${detail ?? 'unknown error'}`
    case 'invalid_request':
      return detail ?? 'invalid request'
    case 'invalid_identity':
      return `invalid ${String(value.kind ?? 'identity')}: ${detail ?? 'reason unknown'}`
    case 'identity_unavailable':
      return `cannot allocate identity: ${detail ?? 'unknown error'}`
    case 'identity_exhausted':
      return `${String(value.kind ?? 'identity')} identity space is exhausted`
    case 'service_unavailable':
      return `the ${String(value.service ?? 'required')} service is unavailable`
    case 'shell_timeout':
      return `shell command ${String(value.command ?? '?')} timed out`
    case 'unexpected_response':
      return `unexpected response: ${detail ?? 'none'}`
    case 'capture_already_active':
    case 'path_not_unique':
      return `${humanize(code)}: ${String(value.path ?? 'unknown path')}`
    case 'malformed_chunk':
      return `log ${String(value.id ?? '?')}: malformed chunk at offset ${String(value.offset ?? '?')} (count ${String(value.count ?? '?')})`
    case 'no_ack':
      return `no reply to ${String(value.what ?? 'the request')} after the retry budget`
  }
  const known = OPERATION[code] ?? RUNTIME[code] ?? MOTOR_PROTOCOL[code] ?? CAPTURE[code] ?? LOG_PROTOCOL[code]
  if (known) return detail ? `${known}: ${detail}` : known
  return detail ? `${humanize(code)}: ${detail}` : humanize(code)
}

/**
 * Format a rejected `actuator_*`/log IPC call for the operator.
 *
 * Strings pass through unchanged; a serialized operation error is rendered as
 * its `thiserror` reason instead of `[object Object]`.
 */
export function describeOperationFailure(error: unknown): string {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  if (!isRecord(error)) return String(error)
  return describe(error, 0)
}
