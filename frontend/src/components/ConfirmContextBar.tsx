// ConfirmContextBar (plan §7 U4): the state a flight command is about to run
// against — flight mode, height above ground, battery — shown directly above
// every medium-risk command, so "hold to land" is confirmed against real
// numbers rather than memory.
//
// The height is ground-relative, and the ground is resolved the same way as
// the map readout (`cesium/heights.ts`): the DEM/3D mesh under the vehicle
// first, then the HOME plane. A HOME-plane fallback is not AGL above the
// terrain, so it is only used when no surface is loaded at all.

import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { heightAboveGroundM } from '../cesium/heights'
import { ConfirmBar } from '../design-system/components'
import { useCommandStore } from '../stores/command'
import { useMissionStore } from '../stores/mission'
import { useTelemetryStore } from '../stores/telemetry'
import { useTerrainStore } from '../stores/terrain'
import { modeLabel } from '../util/modeLabel'

/** Re-sample the ground under the vehicle at most this often, or this far. */
const GROUND_SAMPLE_MS = 2000
const GROUND_SAMPLE_MOVE_M = 25

/** Battery tint thresholds — the same ones the nav bar uses (TopBar). */
const LOW_BATTERY_PCT = 30

export default function ConfirmContextBar({ note }: { note?: string | null }) {
  const { t } = useTranslation()
  const heartbeat = useTelemetryStore((s) => s.snapshot?.heartbeat ?? null)
  const position = useTelemetryStore((s) => s.snapshot?.global_position ?? null)
  const batteryPct = useTelemetryStore(
    (s) => s.snapshot?.sys_status?.battery_remaining_percent ?? null,
  )
  const planHomeAmslM = useMissionStore((s) => s.home?.[2] ?? 0)
  const setHomeAmslM = useCommandStore((s) => s.homeMarker?.[2] ?? null)
  const [groundAmslM, setGroundAmslM] = useState<number | null>(null)
  const last = useRef<{ at: number; lat: number; lon: number } | null>(null)

  const lat = position?.latitude_deg ?? null
  const lon = position?.longitude_deg ?? null

  useEffect(() => {
    if (lat === null || lon === null) {
      last.current = null
      setGroundAmslM(null)
      return
    }
    const now = Date.now()
    const prev = last.current
    const movedM = prev
      ? Math.hypot((lat - prev.lat) * 111_320, (lon - prev.lon) * 111_320 * Math.cos((lat * Math.PI) / 180))
      : Infinity
    if (prev && now - prev.at < GROUND_SAMPLE_MS && movedM < GROUND_SAMPLE_MOVE_M) return
    last.current = { at: now, lat, lon }
    let cancelled = false
    void (async () => {
      let ground: number | null = null
      try {
        ground = await useTerrainStore.getState().samplePointAmsl(lat, lon)
      } catch {
        ground = null // no terrain: fall back to the HOME plane below
      }
      if (!cancelled) setGroundAmslM(ground)
    })()
    return () => {
      cancelled = true
    }
  }, [lat, lon])

  const aglM =
    position === null
      ? null
      : heightAboveGroundM({
          demGroundAmslM: groundAmslM,
          planHomeAmslM: planHomeAmslM !== 0 ? planHomeAmslM : (setHomeAmslM ?? 0),
          reportedAmslM: position.altitude.meters,
          relativeAltM: position.relative_alt_m,
        })

  return (
    <ConfirmBar
      fields={[
        { key: 'mode', label: t('link.mode'), value: modeLabel(t, heartbeat) },
        { key: 'agl', label: t('plan.profile.agl'), value: aglM === null ? null : `${aglM.toFixed(1)} m` },
        {
          key: 'battery',
          label: t('link.batt'),
          value: batteryPct === null ? null : `${Math.round(batteryPct)}%`,
          tone: batteryPct !== null && batteryPct < LOW_BATTERY_PCT ? 'warn' : undefined,
        },
      ]}
      note={note}
    />
  )
}
