import { useEffect, useState } from 'react'

/** The width at or below which the TopBar folds its low-priority chips
 * (plan §7 U2: "at ≤1366 narrow the low-priority pills fold into 'More'").
 * Matches `narrowScreen()` in `stores/ui.ts`. */
export const NARROW_MAX_PX = 1366

/**
 * Reactive width breakpoint (defaults to NARROW_MAX_PX). The ui store's own
 * `narrowScreen()` is only read once at boot; the TopBar has to react when the
 * window is resized between the two screenshot widths (§7.4).
 */
export function useIsNarrow(maxWidth = NARROW_MAX_PX): boolean {
  const query = `(max-width: ${maxWidth}px)`
  const [narrow, setNarrow] = useState(
    () => typeof window !== 'undefined' && window.matchMedia(query).matches,
  )
  useEffect(() => {
    const mq = window.matchMedia(query)
    const onChange = () => setNarrow(mq.matches)
    onChange()
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [query])
  return narrow
}
