// Mission sync status: the single answer to "does the FC hold this plan?".
//
// The store keeps the raw facts (`dirty`, `fcMatches`, `lastSyncedHash`); this
// module turns them into one status the planning panel renders, so the wording
// cannot drift between call sites. `check:mission-sync` drives the store through
// the real upload/download event sequences and asserts these statuses.

/** Relationship between the local plan and the FC's mission. */
export type PlanSyncStatus =
  /** Nothing to sync (no waypoints, no complex-item children). */
  | 'empty'
  /** Local edits that have never been sent to an FC. */
  | 'unsynced'
  /** Edited since the last successful sync with the FC. */
  | 'dirty'
  /** The FC read-back differs from what we uploaded. */
  | 'mismatch'
  /** The local plan is known to match the FC. */
  | 'synced'

export interface PlanSyncFacts {
  /** Flyable item count (editable waypoints + complex-item children). */
  itemCount: number
  /** True when the plan was edited since the last sync. */
  dirty: boolean
  /** Post-upload read-back result; null = never checked. */
  fcMatches: boolean | null
  /** Hash of the items at the last successful sync; null = never synced. */
  lastSyncedHash: string | null
}

/**
 * Classify the sync state.
 *
 * A failed upload or read-back must never be reported as "unsaved" alone: the
 * `dirty` flag is only meaningful once there is a baseline (`lastSyncedHash`),
 * otherwise the plan was simply never uploaded (issues.md #33).
 */
export function planSyncStatus(f: PlanSyncFacts): PlanSyncStatus {
  if (f.itemCount === 0) return 'empty'
  if (f.fcMatches === false) return 'mismatch'
  const synced = f.lastSyncedHash !== null
  if (f.dirty) return synced ? 'dirty' : 'unsynced'
  return synced ? 'synced' : 'unsynced'
}
