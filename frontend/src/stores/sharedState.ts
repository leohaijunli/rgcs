// A11 phase 1: the frontend projection of the core's shared state. Windows
// never own this state — they read it, write with the version they read, and
// re-project the broadcast. A conflict means another window wrote in
// between: the caller re-reads and retries once (last-writer-wins is NOT the
// rule; the version gate makes the loser see the winner's value).
//
// Migrated slices (phase 1): the map layer toggles (`layers.*`). The pattern
// for the next slices (selection, camera, drape result) is the same:
// `useSharedValue('key')` for display, `writeShared('key', value, version)`
// for edits.

import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isTauri } from '../inspector/mock'

export interface SharedEntry<T = unknown> {
  version: number
  value: T
  written_by: string
}

type ChangePayload = { key: string; entry: SharedEntry }

/** The process-local mirror, hydrated once and updated by the broadcast. */
let mirror: Map<string, SharedEntry> = new Map()
let bridgeStarted = false
const listeners = new Set<() => void>()

function notify(): void {
  for (const l of listeners) l()
}

async function startBridge(): Promise<void> {
  if (bridgeStarted || !isTauri()) return
  bridgeStarted = true
  try {
    const snap = await invoke<Record<string, SharedEntry>>('shared_state_snapshot')
    mirror = new Map(Object.entries(snap))
    notify()
    await listen<ChangePayload>('shared_state_changed', (e) => {
      mirror.set(e.payload.key, e.payload.entry)
      notify()
    })
  } catch {
    // Browser mock: no shell — the hook reports "unset" and writes no-op.
  }
}

void startBridge()

/** Subscribe to the mirror for a key (re-renders on every broadcast). */
export function useSharedValue<T>(key: string): SharedEntry<T> | null {
  const [, bump] = useState(0)
  useEffect(() => {
    const l = () => bump((n) => n + 1)
    listeners.add(l)
    return () => {
      listeners.delete(l)
    }
  }, [])
  return (mirror.get(key) as SharedEntry<T> | undefined) ?? null
}

/** The current version for a key (`0` = never written; a bootstrap write
 * passes `0` which the core treats as create). */
export function sharedVersion(key: string): number {
  return mirror.get(key)?.version ?? 0
}

/**
 * Write a key, retrying once on a version conflict with the fresh version.
 *
 * @returns the accepted entry's version, or `null` when running without a
 *   shell (browser mock).
 */
export async function writeShared<T>(key: string, value: T): Promise<number | null> {
  if (!isTauri()) return null
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const update = await invoke<ChangePayload>('shared_state_set', {
        key,
        value,
        expectedVersion: sharedVersion(key),
        writer: 'main',
      })
      mirror.set(update.key, update.entry)
      notify()
      return update.entry.version
    } catch {
      // Conflict or transport hiccup: re-read via the snapshot and retry once.
      try {
        const snap = await invoke<Record<string, SharedEntry>>('shared_state_snapshot')
        mirror = new Map(Object.entries(snap))
        notify()
      } catch {
        return null
      }
    }
  }
  return null
}
