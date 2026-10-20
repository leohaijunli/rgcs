// Four-level link model (ADR-011), shared by the top bar and the settings page.
//
// The transport state and the target-FC heartbeat disagree on purpose: a bound
// UDP socket keeps receiving foreign traffic while our FC is silent, so a
// single connected flag would hide a dead FC behind unrelated packets.

import type { LinkStatus } from '../generated-types/LinkStatus'

export type LinkLevel = 'disconnected' | 'listening' | 'noFc' | 'online'

/**
 * 1. disconnected — no socket bound.
 * 2. listening   — socket bound (or binding), no inbound telemetry yet.
 * 3. noFc        — telemetry arriving but no target-FC HEARTBEAT.
 * 4. online      — target-FC HEARTBEAT fresh.
 */
export function linkLevel(link: LinkStatus | null, hasPackets: boolean): LinkLevel {
  if (!link || link.link_state === 'disconnected') return 'disconnected'
  if (link.link_state !== 'connected') return 'listening'
  if (link.fc_alive) return 'online'
  return hasPackets ? 'noFc' : 'listening'
}

/** Tone for the four levels (state colours only, never decoration — §6). */
export function linkLevelTone(level: LinkLevel): 'ok' | 'warn' | 'err' | 'off' {
  switch (level) {
    case 'online':
      return 'ok'
    case 'noFc':
      return 'warn'
    case 'listening':
      return 'off'
    default:
      return 'err'
  }
}

const FIELD_AGE_KEYS = [
  'heartbeat_at_ms',
  'global_position_at_ms',
  'attitude_at_ms',
  'sys_status_at_ms',
  'battery_at_ms',
  'gps_at_ms',
] as const

/** True once any telemetry field has been seen since connecting. */
export function hasInboundPackets(
  fieldAges: Record<string, number | null> | undefined | null,
): boolean {
  if (!fieldAges) return false
  return FIELD_AGE_KEYS.some((k) => fieldAges[k] != null)
}

/** Age in ms of the most recently updated telemetry field, or `null`. */
export function lastPacketAgeMs(
  fieldAges: Record<string, number | null> | undefined | null,
  nowMs: number = Date.now(),
): number | null {
  if (!fieldAges) return null
  let newest: number | null = null
  for (const k of FIELD_AGE_KEYS) {
    const at = fieldAges[k]
    if (at != null && (newest === null || at > newest)) newest = at
  }
  return newest === null ? null : Math.max(0, nowMs - newest)
}
