import { useEffect, useRef } from 'react'
import { useCesiumViewer } from '../hooks/useCesiumViewer'
import { useMissionStore } from '../stores/mission'
import { useTelemetryStore } from '../stores/telemetry'
import MapToolbar from './MapToolbar'

/** Composition shell: viewer lifecycle + store wiring + chrome (issues.md #29). */
export default function MapView() {
  const containerRef = useRef<HTMLDivElement>(null)
  const { initError, setSnapshot, setMission, goHome } = useCesiumViewer(containerRef)

  // Telemetry updates: drone position, trail, camera follow.
  const snapshot = useTelemetryStore((s) => s.snapshot)
  useEffect(() => {
    setSnapshot(snapshot)
  }, [snapshot, setSnapshot])

  // Mission waypoints: render a polyline + numbered points, re-run when items change.
  const missionItems = useMissionStore((s) => s.items)
  const selectedSeq = useMissionStore((s) => s.selectedSeq)
  useEffect(() => {
    setMission(missionItems, selectedSeq)
  }, [missionItems, selectedSeq, setMission])

  if (initError) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-2 p-6 text-center">
        <div className="text-sm font-medium text-error">Map failed to initialize</div>
        <pre className="mono max-w-full overflow-auto rounded border border-line bg-canvas p-3 text-xs text-ink">
          {initError}
        </pre>
      </div>
    )
  }

  return (
    <div className="relative h-full w-full">
      <div ref={containerRef} className="h-full w-full" />
      <MapToolbar onGoHome={goHome} onToggleMeasure={() => undefined} />
    </div>
  )
}
