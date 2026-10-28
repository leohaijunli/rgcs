// Height above the DTM under the vehicle (DR6): the anchored AMSL minus the
// ground under it — the same frame the plan was draped in, unlike REL which is
// home-relative. Shared by the HUD readout and the "change altitude" command
// dialog, so the number an operator commands against is the number the HUD
// shows.

import { useEffect, useMemo, useState } from 'react'
import { heightAboveGroundM, vehicleAmslAnchoredM } from '../cesium/heights'
import { useTelemetryStore } from '../stores/telemetry'
import { useTerrainStore } from '../stores/terrain'

/** Own-sampler debounce when the map's ground effect has no value yet
 * (`?nomap` harness runs). */
const SAMPLE_DEBOUNCE_MS = 500

export function useVehicleAgl(): number | null {
  const snapshot = useTelemetryStore((s) => s.snapshot)
  const samplePointAmsl = useTerrainStore((st) => st.samplePointAmsl)
  const vehicleGround = useTerrainStore((st) => st.vehicleGroundAmslM)
  const [sampledGround, setSampledGround] = useState<number | null>(null)
  const pos = snapshot?.global_position ?? null
  // The map's ground effect maintains `vehicleGroundAmslM` continuously; the
  // sampler only runs when that is missing.
  const groundAmsl = vehicleGround ?? sampledGround

  const aglM = useMemo(() => {
    if (pos == null || groundAmsl == null) return null
    const anchored = vehicleAmslAnchoredM({
      anchorOffsetM: useTerrainStore.getState().amslAnchorOffsetM,
      demGroundAtHomeM: useTerrainStore.getState().homeGroundAmsl,
      reportedAmslM: pos.altitude.meters,
      relativeAltM: pos.relative_alt_m,
    })
    return heightAboveGroundM({
      demGroundAmslM: groundAmsl,
      planHomeAmslM: 0,
      reportedAmslM: anchored,
      relativeAltM: pos.relative_alt_m,
    })
  }, [pos, groundAmsl])

  useEffect(() => {
    if (pos == null || vehicleGround != null) return
    const id = setTimeout(() => {
      void samplePointAmsl(pos.latitude_deg, pos.longitude_deg).then((g) => setSampledGround(g))
    }, SAMPLE_DEBOUNCE_MS)
    return () => clearTimeout(id)
  }, [pos?.latitude_deg, pos?.longitude_deg, samplePointAmsl, vehicleGround])

  return aglM
}
