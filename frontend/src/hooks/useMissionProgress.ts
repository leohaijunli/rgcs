// Mission progress derived from the current plan and the latest fix
// (issues.md #37). Shared by the flight inspector and the Missions drawer so
// both readouts agree on which waypoint is active and how far it is.

import { useMemo } from 'react'
import { compileWaypoints, toDisplayItems } from '../mission/compile'
import { orderedMissionItems } from '../mission/planfile'
import {
  missionProgress,
  progressItemsOf,
  type MissionProgress,
  type ProgressItem,
} from '../mission/progress'
import { useMissionStore } from '../stores/mission'
import { useTelemetryStore } from '../stores/telemetry'

export interface MissionProgressState {
  /** Plan in flyable order, coordinate items only. */
  items: ProgressItem[]
  progress: MissionProgress
}

export function useMissionProgress(): MissionProgressState {
  const waypoints = useMissionStore((s) => s.waypoints)
  const altitudeMode = useMissionStore((s) => s.altitudeMode)
  const home = useMissionStore((s) => s.home)
  const blocks = useMissionStore((s) => s.blocks)
  const planBase = useMissionStore((s) => s.planBase)
  const currentSeq = useMissionStore((s) => s.currentSeq)
  const pos = useTelemetryStore((s) => s.snapshot?.global_position ?? null)
  const homeAmsl = home?.[2] ?? 0

  const items = useMemo(
    () =>
      progressItemsOf(
        toDisplayItems(
          orderedMissionItems(
            compileWaypoints(waypoints, altitudeMode, homeAmsl),
            blocks,
            planBase ?? undefined,
          ),
          homeAmsl,
        ),
      ),
    [waypoints, altitudeMode, homeAmsl, blocks, planBase],
  )

  // Primitives so the memo keys are stable across telemetry frames.
  const lat = pos?.latitude_deg ?? null
  const lon = pos?.longitude_deg ?? null
  const speedMs = pos ? Math.hypot(pos.velocity.x_m_s, pos.velocity.y_m_s) : null

  const progress = useMemo(() => {
    const vehicle = lat === null || lon === null ? null : { latitude_deg: lat, longitude_deg: lon }
    return missionProgress(items, currentSeq, vehicle, speedMs)
  }, [items, currentSeq, lat, lon, speedMs])

  return { items, progress }
}
