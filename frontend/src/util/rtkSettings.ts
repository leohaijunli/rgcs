import type { BaseSerialRequest } from '../generated-types/BaseSerialRequest'

export const RTK_SETTINGS_KEY = 'maggcs.rtk.survey-in'
export const SURVEY_DEFAULTS = { accuracy_limit_m: 2, min_duration_s: 180 } as const
export const SURVEY_LIMITS = { minAccuracy: 0.01, maxAccuracy: 5, minDuration: 1, maxDuration: 360 } as const

/** Import the old webview-only receiver preferences before first auto start. */
export function legacyRtkSettings(): BaseSerialRequest | null {
  try {
    const value = JSON.parse(localStorage.getItem(RTK_SETTINGS_KEY) ?? 'null') as Record<string, unknown> | null
    if (!value) return null
    const baud = Number(value.baud)
    if (![0, 9600, 38400, 57600, 115200, 230400, 460800, 921600].includes(baud)) return null
    if (value.kind === 'preconfigured') return baud === 0 ? null : { baud: { kind: 'fixed', rate: baud }, mode: { kind: 'preconfigured' } }
    const accuracy = Number(value.accuracy)
    const duration = Number(value.duration)
    if (value.kind !== 'survey_in' || !Number.isFinite(accuracy) || accuracy < SURVEY_LIMITS.minAccuracy || accuracy > SURVEY_LIMITS.maxAccuracy
      || !Number.isInteger(duration) || duration < SURVEY_LIMITS.minDuration || duration > SURVEY_LIMITS.maxDuration) return null
    return { baud: baud === 0 ? { kind: 'auto' } : { kind: 'fixed', rate: baud },
      mode: { kind: 'survey_in', config: { accuracy_limit_m: accuracy, min_duration_s: duration } } }
  } catch { return null }
}
