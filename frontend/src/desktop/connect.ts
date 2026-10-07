// Thin wrapper over the Tauri `connect` command.
import { invoke } from '@tauri-apps/api/core'
import type { LinkStatus } from '../generated-types/LinkStatus'
import { useLinkStore } from '../stores/link'

/** Ask the desktop shell to open a MAVLink connection. */
export async function connectEndpoint(endpoint: string): Promise<LinkStatus | null> {
  await invoke('connect', { endpoint })
  return invoke<LinkStatus | null>('link_status')
}

/** Ask the desktop shell to close the active connection. */
export async function disconnect(): Promise<void> {
  await invoke('disconnect')
}

/** Ask the desktop shell to stop the link and quit the application. */
export async function shutdownApp(): Promise<void> {
  await invoke('shutdown_app')
}

/** Present a connection error to the user. */
export function reportConnectError(e: unknown): void {
  useLinkStore.getState().pushError({
    kind: 'other',
    message: e instanceof Error ? e.message : String(e),
    at_ms: Date.now(),
  })
}
