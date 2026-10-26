/** Replay seek math. The ULog source reads forward-only, so a seek to
 * `targetMs` must start `spanMs` earlier and stream forward; the backend's
 * unpaced scrub then fills exactly one visible window and lands the playhead
 * on the dragged position. Returns `(sourceSeekMs, scrubSpanMs)`. */
export function replaySeekWindow(targetMs: number, windowMs: number): { sourceSeekMs: number; scrubSpanMs: number } {
  const span = Math.max(0, windowMs)
  return { sourceSeekMs: Math.max(0, targetMs - span), scrubSpanMs: span }
}