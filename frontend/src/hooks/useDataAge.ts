import { useEffect, useState } from 'react'
import { useTelemetryStore } from '../stores/telemetry'

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