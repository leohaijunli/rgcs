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

/** U3: a Warning is lower-pitched and single; a Critical is a double high tone
 * so the two are distinguishable without looking at the screen. */
export const WARN_FREQ_HZ = 660
export const CRITICAL_FREQ_HZ = 880
/** Gap between the two beeps of the critical pattern, ms. */
export const CRITICAL_GAP_MS = 170
/** Length of one beep inside an alert pattern, ms. */
export const ALERT_BEEP_MS = 130

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

/**
 * Severity sound pattern for the alert center (U3): Critical is a double high
 * beep, Warning a single lower one, Info is silent. `volume` is 0..1 and
 * scales the shared alarm gain; there is one timbre, not a bundled asset.
 */
export function alertBeep(severity: 'critical' | 'warn' | 'info', volume = 1): void {
  const gain = ALARM_GAIN * Math.max(0, Math.min(1, volume))
  if (severity === 'critical') {
    beep(CRITICAL_FREQ_HZ, ALERT_BEEP_MS, gain)
    setTimeout(() => beep(CRITICAL_FREQ_HZ, ALERT_BEEP_MS, gain), CRITICAL_GAP_MS)
  } else if (severity === 'warn') {
    beep(WARN_FREQ_HZ, ALERT_BEEP_MS, gain)
  }
}

