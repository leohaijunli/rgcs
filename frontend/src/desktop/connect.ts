// Thin wrapper over the Tauri `connect` command.
import { invoke } from '@tauri-apps/api/core'
import type { LinkStatus } from '../generated-types/LinkStatus'
import type { StopReason } from '../generated-types/StopReason'
import type { TeardownGuard } from '../generated-types/TeardownGuard'
import { useLinkStore } from '../stores/link'

/** Ask the desktop shell to open a MAVLink connection. */
export async function connectEndpoint(endpoint: string): Promise<LinkStatus | null> {
  await invoke('connect', { endpoint })
  return invoke<LinkStatus | null>('link_status')
}

/**
 * Ask the desktop shell to close the active connection.
 *
 * Runs the link-scoped teardown supervisor (A1). Rejects with the reason when
 * the supervisor refuses (the vehicle is armed or airborne, L4) — the shell
 * also raises a `warn` alert, so the reason is visible even if a caller
 * swallows the error.
 */
export async function disconnect(): Promise<void> {
  await invoke('disconnect')
}

/**
 * Ask the desktop shell to stop the link and quit the application.
 *
 * The full teardown (A1). Rejects with the refusal reason while the vehicle
 * is armed or airborne (L4); closing the window is the forced exit.
 */
export async function shutdownApp(): Promise<void> {
  await invoke('shutdown_app')
}

/**
 * What a teardown would do right now (L4), asked *before* the operator
 * commits: the refusal reason, the word to type, and whether this reason is
 * the forced window-chrome escape hatch.
 *
 * `atStake` is this window's own state — plan work the FC does not have.
 */
export async function teardownGuard(
  reason: StopReason,
  atStake: boolean,
): Promise<TeardownGuard> {
  return invoke<TeardownGuard>('teardown_guard', { reason, atStake })
}

/**
 * Run a confirmed teardown. `window_close` is not an in-app action: the shell
 * owns the window chrome, and asking for it here would be a lie about where
 * the shutdown came from.
 */
export async function runTeardown(reason: StopReason): Promise<void> {
  switch (reason) {
    case 'disconnect':
      return disconnect()
    case 'shutdown_app':
      return shutdownApp()
    default:
      throw new Error(`teardown ${reason} is not an in-app action`)
  }
}

/** Present a connection error to the user. */
export function reportConnectError(e: unknown): void {
  useLinkStore.getState().pushError({
    kind: 'other',
    message: e instanceof Error ? e.message : String(e),
    at_ms: Date.now(),
  })
}
