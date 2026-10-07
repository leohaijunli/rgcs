import { useEffect, useState } from 'react'
import { useTelemetryStore } from '../stores/telemetry'

/** A field older than this is shown as stale (issues.md #16). */
export const FIELD_STALE_MS = 2000

function perfNow(): number {
  return typeof performance !== 'undefined' ? performance.now() : 0
}

/**
 * Ticks every `intervalMs` and returns the age (ms) of the latest telemetry
 * snapshot. Infinity when no snapshot has arrived yet.
 */
export function useDataAge(intervalMs = 1000): number {
  const lastUpdateAtMs = useTelemetryStore((s) => s.lastUpdateAtMs)
  const [now, setNow] = useState(perfNow)
  useEffect(() => {
    const id = window.setInterval(() => setNow(perfNow()), intervalMs)
    return () => window.clearInterval(id)
  }, [intervalMs])
  if (lastUpdateAtMs === 0) return Number.POSITIVE_INFINITY
  return now - lastUpdateAtMs
}

/**
 * Age (ms) of one telemetry field, given the backend's epoch-ms timestamp for
 * it. Returns `Infinity` when the field has not been seen since connecting.
 *
 * The timestamp comes from the same machine, so comparing it to `Date.now()` is
 * safe; the interval keeps the age growing even if no new snapshot arrives.
 */
export function useFieldAge(
  atMs: number | null | undefined,
  intervalMs = 500,
): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), intervalMs)
    return () => window.clearInterval(id)
  }, [intervalMs])
  if (atMs == null) return Number.POSITIVE_INFINITY
  return Math.max(0, now - atMs)
}
