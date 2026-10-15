// Alert sound preferences (plan §3 U3): severity sounds have one volume, and
// muting is time-boxed — the operator can silence the alarm for a while, and
// it comes back on its own, because "muted" left forever is exactly how a
// flight loses its audible warning. Persisted in localStorage like the other
// desktop prefs.

import { useEffect, useState } from 'react'
import { create } from 'zustand'

const VOLUME_KEY = 'maggcs.alertVolume'
/** Default volume (0..1) — audible outdoors without pinning the meter. */
export const DEFAULT_ALERT_VOLUME = 0.6
/** How long a mute lasts before the alarm re-arms itself. */
export const MUTE_MS = 5 * 60 * 1000

function readVolume(): number {
  if (typeof localStorage === 'undefined') return DEFAULT_ALERT_VOLUME
  const raw = Number(localStorage.getItem(VOLUME_KEY))
  return Number.isFinite(raw) && raw >= 0 && raw <= 1 ? raw : DEFAULT_ALERT_VOLUME
}

interface AlertSoundState {
  /** 0..1, scales every severity sound. */
  volume: number
  /** Epoch ms until which sound is muted; 0 when not muted. */
  mutedUntil: number
  setVolume: (v: number) => void
  /** Silence for `ms` (default [`MUTE_MS`]); auto-restores when it elapses. */
  mute: (ms?: number) => void
  unmute: () => void
}

export const useAlertSound = create<AlertSoundState>((set) => ({
  volume: readVolume(),
  mutedUntil: 0,
  setVolume: (v) => {
    const volume = Math.max(0, Math.min(1, v))
    if (typeof localStorage !== 'undefined') localStorage.setItem(VOLUME_KEY, String(volume))
    set({ volume })
  },
  mute: (ms = MUTE_MS) => set({ mutedUntil: Date.now() + ms }),
  unmute: () => set({ mutedUntil: 0 }),
}))

/** True while the timed mute is active; ticks so the UI can count down and so
 * the alarm re-arms the moment the window lapses. */
export function useAlertMuted(intervalMs = 1000): { muted: boolean; remainingMs: number } {
  const mutedUntil = useAlertSound((s) => s.mutedUntil)
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), intervalMs)
    return () => window.clearInterval(id)
  }, [intervalMs])
  const remainingMs = mutedUntil - now
  return { muted: remainingMs > 0, remainingMs: Math.max(0, remainingMs) }
}

/** Whether the OS asks us to avoid motion (the flash honours this). */
export function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches
}
