// Synthesized alarm tones (WebAudio) — no bundled asset, works on Ubuntu and
// Windows alike. Extracted from the actuator alarm's beep so the heartbeat
// banner (plan §4 L3) and, later, the alert center's severity sounds (U3)
// share one implementation.

/** Square-wave frequency of the plain alarm tone, Hz. */
export const ALARM_FREQ_HZ = 880
/** Duration of one beep, ms. */
export const ALARM_BEEP_MS = 180
/** Peak gain: loud enough to notice, quiet enough not to startle. */
export const ALARM_GAIN = 0.08

let ctxRef: AudioContext | null = null

/**
 * Play one alarm beep. Best-effort by design: a machine without an audio
 * device (or an autoplay-blocked context) must never break the visual alarm.
 */
export function beep(
  freqHz = ALARM_FREQ_HZ,
  durationMs = ALARM_BEEP_MS,
  gainValue = ALARM_GAIN,
): void {
  try {
    ctxRef ??= new AudioContext()
    const ctx = ctxRef
    const osc = ctx.createOscillator()
    const gain = ctx.createGain()
    osc.type = 'square'
    osc.frequency.value = freqHz
    gain.gain.value = gainValue
    osc.connect(gain).connect(ctx.destination)
    osc.start()
    osc.stop(ctx.currentTime + durationMs / 1000)
  } catch {
    /* no audio device: the visual alarm still runs */
  }
}
