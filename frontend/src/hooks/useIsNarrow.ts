import { useEffect, useState } from 'react'

/** The width at or below which the TopBar folds its low-priority chips
 * (plan §7 U2: "at ≤1366 narrow the low-priority pills fold into 'More'").
 * Matches `narrowScreen()` in `stores/ui.ts`. */
export const NARROW_MAX_PX = 1366

const QUERY = `(max-width: ${NARROW_MAX_PX}px)`

/**
 * Reactive `window.innerWidth <= NARROW_MAX_PX`. The ui store's own
 * `narrowScreen()` is only read once at boot; the TopBar has to react when the
 * window is resized between the two screenshot widths (§7.4).
 */
export function useIsNarrow(): boolean {
  const [narrow, setNarrow] = useState(
    () => typeof window !== 'undefined' && window.matchMedia(QUERY).matches,
  )
  useEffect(() => {
    const mq = window.matchMedia(QUERY)
    const onChange = () => setNarrow(mq.matches)
    onChange()
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [])
  return narrow
}
